#!/usr/bin/env node
/**
 * The server's crates as Cargo builds them. `check-module-layers.mjs` holds
 * the modules still in `server-rs/src` to the crates they are going to; this
 * holds the workspace's members to `server-rs/layers.json`, read from
 * `cargo metadata --no-deps`: the declared dependencies, whatever a manifest's
 * spelling (renamed, per target, inherited from the workspace), with no
 * resolution and no network.
 *
 * - Every member is a crate of a layer, or one of the crates `outside` lists
 *   (the relay and the developer kit), not both. A crate layers.json declares
 *   that is no member yet (its code is still a module of the server, or is
 *   still to come) is left out of every rule below until it is one.
 * - A crate of the layers depends on its own layer or a lower one: on a crate
 *   of an open layer below its own, on one its `mayUse` lists, or, for the
 *   entry (`mayUse: "*"`), on any crate below it. These are the module rules,
 *   held to `[dependencies]`. It depends on no crate outside the layers, and a
 *   crate outside them depends only on the open layers and on one another.
 * - No member depends on the entry (the crate whose `mayUse` is `*`) in any
 *   way: not even its tests may need the whole server.
 * - No crate of a `forbidden` rule's `from` reaches one of its `to` through
 *   any chain of dependencies, its own dev- and build-dependencies included,
 *   and no `mayUse` lists such a pair. `@<layer>` stands for a layer's
 *   crates; a pair the rule's `except` lists is let through. A failure gives
 *   the chain and the rule's reason. A rule with no pair whose crates are
 *   both members yet is waiting, and is listed so.
 * - Only the members a `wrappers` entry lists depend on its external crate,
 *   and each of them still does: the list only shrinks.
 *
 * A dependency is one of `[dependencies]`, per target too and optional ones
 * included: what a crate links into the program. Dev-dependencies link only
 * into the crate's own tests and build-dependencies only into its build
 * script, and Cargo lets either point anywhere in the workspace, up a layer
 * too: the layer and wrapper rules leave them out. The forbidden pairs and
 * the entry's rule do not. A test of the provider base on a real provider,
 * or of one provider on another, brings back the coupling the crates keep
 * apart, and links a second copy of the crate under test into it.
 *
 * It also prints what the layers are judged by: the largest crate cycle with
 * the dev-dependencies (Cargo refuses one in `[dependencies]`), each middle
 * crate's code lines and how many members depend on it, and the matches of
 * `Source::Claude|Codex|Acp` outside the provider crates, counted as
 * `check-module-layers.mjs` counts them while the server still holds
 * modules that are going to other crates.
 *
 *   node scripts/check-crate-layers.mjs [--json]     (pnpm check:crate-layers)
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { checkLayers, cycles, moduleGraph, workspaceCrates } from "./check-module-layers.mjs";

/**
 * The members of a workspace from its `cargo metadata` (format 1):
 * `Map<name, { name, manifest, deps: [{ name, member, kind, as }] }>`.
 * A dependency is on a `member` when it is a path dependency into a member's
 * directory; `kind` is `normal`, `dev` or `build`; `as` says it as the
 * manifest does (renamed, per target).
 */
export function members(meta) {
  const ids = new Set(meta.workspace_members);
  const packages = meta.packages.filter((p) => ids.has(p.id));
  const byDir = new Map(packages.map((p) => [dirname(p.manifest_path), p.name]));
  const root = meta.workspace_root ?? "";
  return new Map(
    packages.map((p) => [
      p.name,
      {
        name: p.name,
        manifest: root ? relative(root, p.manifest_path) : p.manifest_path,
        deps: (p.dependencies ?? []).map((d) => {
          const kind = d.kind ?? "normal";
          const as = `${kind === "normal" ? "" : `${kind}-`}dependency ${d.rename ? `${d.rename} (${d.name})` : d.name}${d.target ? ` for ${d.target}` : ""}`;
          return { name: d.name, member: d.path != null ? byDir.get(d.path) : undefined, kind, as };
        }),
      },
    ]),
  );
}

/** `@<layer>` as that layer's crates, any other name as itself. */
const expand = (names, layers) =>
  names.flatMap((n) => (n.startsWith("@") ? (layers.find((l) => l.name === n.slice(1))?.crates ?? []).map((c) => c.name) : [n]));

/** The shortest chain of `edges` from `from` to each crate it reaches: `Map<to, [from, …, to]>`. */
function chains(from, edges) {
  const found = new Map([[from, [from]]]);
  const queue = [from];
  while (queue.length) {
    const at = queue.shift();
    for (const next of edges.get(at) ?? []) {
      if (found.has(next)) continue;
      found.set(next, [...found.get(at), next]);
      queue.push(next);
    }
  }
  found.delete(from);
  return found;
}

/**
 * The workspace's members (`meta`, `cargo metadata` format 1) checked against
 * `config` (`layers.json`): its `layers`, and
 * - `outside`, `{ crates: [names] }`: members that are not the server's;
 * - `forbidden`, each `{ name, why, from: [names], to: [names], except?: ["a -> b"] }`;
 * - `wrappers`, each `{ crate, why, wrappers: [names] }`: the members that may
 *   depend on the external `crate`.
 * Returns `{ problems, stats }`; `sizes` (`Map<name, code lines>`) and
 * `sourceMatches` only go into `stats`.
 */
export function checkCrates(meta, config, { sizes = new Map(), sourceMatches } = {}) {
  const problems = [];
  const all = members(meta);
  const layers = config.layers ?? [];
  const placed = new Map();
  layers.forEach((layer, level) => {
    for (const c of layer.crates ?? []) {
      placed.set(c.name, { name: c.name, layer: layer.name, level, open: Boolean(layer.open), middle: Boolean(layer.middle), mayUse: c.mayUse ?? [] });
    }
  });
  const outside = new Set(config.outside?.crates ?? []);
  const listed = (name) => placed.has(name) || outside.has(name);

  // ------------------------------------------------------------ placement
  for (const name of outside) {
    if (placed.has(name)) problems.push(`layers.json places ${name} in a layer and lists it outside the layers: one or the other`);
  }
  for (const m of all.values()) {
    if (!listed(m.name)) problems.push(`${m.manifest}: ${m.name} is a workspace member that layers.json neither places in a layer nor lists outside the layers`);
  }
  const waiting = [...placed.keys()].filter((name) => !all.has(name));

  // ------------------------------------------------------------ the layer rules
  const mayUse = (x, y) => {
    if (y.level < x.level && (y.open || x.mayUse === "*")) return true;
    return x.mayUse !== "*" && x.mayUse.includes(y.name) && y.level <= x.level;
  };
  const whyNot = (x, y) => (y.level > x.level ? `${y.name} is in a higher layer (${y.layer}) than ${x.name} (${x.layer})` : `${x.name}'s mayUse does not list ${y.name}`);
  const links = new Map([...all.keys()].map((name) => [name, new Set()]));
  const forTests = new Map([...all.keys()].map((name) => [name, new Map()]));
  const withTests = new Map([...all.keys()].map((name) => [name, new Map()]));
  for (const m of all.values()) {
    for (const d of m.deps) {
      if (!d.member || d.member === m.name) continue;
      const to = d.member;
      withTests.get(m.name).set(to, true);
      const where = `${m.manifest}: ${m.name}'s ${d.as}`;
      if (placed.get(to)?.mayUse === "*") {
        problems.push(`${where}: nothing depends on the entry, which wires the crates together; take what is needed from the crate the entry got it from`);
        continue;
      }
      if (d.kind !== "normal") {
        forTests.get(m.name).set(to, d.kind);
        continue;
      }
      links.get(m.name).add(to);
      const [x, y] = [placed.get(m.name), placed.get(to)];
      if (x && y) {
        if (!mayUse(x, y)) problems.push(`${where}: ${whyNot(x, y)}`);
      } else if (x && outside.has(to)) {
        problems.push(`${where}: ${to} is outside the layers, and no crate of the layers depends on one`);
      } else if (outside.has(m.name) && y && !y.open) {
        problems.push(`${where}: a crate outside the layers depends only on the open layers and on one another, and ${to}'s layer (${y.layer}) is not open`);
      }
    }
  }

  // ------------------------------------------------------------ forbidden pairs
  // What a crate reaches in its program, and in its tests and build script:
  // through its own dependencies of any kind, then theirs that link.
  const reach = (from) => {
    const found = chains(from, links);
    for (const [first] of forTests.get(from)) {
      if (found.has(first)) continue;
      found.set(first, [from, first]);
      for (const [to, chain] of chains(first, links)) if (!found.has(to)) found.set(to, [from, ...chain]);
    }
    return found;
  };
  const rules = [];
  for (const [i, rule] of (config.forbidden ?? []).entries()) {
    const label = rule.name?.trim() ? `"${rule.name}"` : `forbidden[${i}]`;
    if (!rule.name?.trim()) problems.push(`layers.json's ${label} has no name`);
    if (!rule.why?.trim()) problems.push(`layers.json's ${label} does not say why`);
    const from = expand(rule.from ?? [], layers);
    const to = expand(rule.to ?? [], layers);
    for (const n of [...(rule.from ?? []), ...(rule.to ?? [])]) {
      if (n.startsWith("@") ? !layers.some((l) => l.name === n.slice(1)) : !listed(n)) problems.push(`layers.json's ${label} names ${n}, which layers.json does not list`);
    }
    if (!from.length || !to.length) problems.push(`layers.json's ${label} forbids nothing: it needs a from and a to`);
    const except = new Set(rule.except ?? []);
    for (const pair of except) {
      const [a, b] = pair.split(" -> ");
      if (!from.includes(a) || !to.includes(b)) problems.push(`layers.json's ${label} lets ${pair} through, which is not one of its pairs`);
    }
    const pairs = from.flatMap((a) => to.filter((b) => a !== b && !except.has(`${a} -> ${b}`)).map((b) => [a, b]));
    for (const [a, b] of pairs) {
      const lists = placed.get(a)?.mayUse;
      if (Array.isArray(lists) && lists.includes(b)) problems.push(`layers.json: ${a}'s mayUse lists ${b}, which ${label} forbids: ${rule.why}`);
    }
    const live = pairs.filter(([a, b]) => all.has(a) && all.has(b));
    rules.push({ label, live: live.length > 0 });
    // One line per crate that reaches what it may not: the nearest such
    // crate with its chain, and the others it reaches through it.
    const reached = new Map();
    for (const [a, b] of live) {
      const chain = reach(a).get(b);
      if (chain) reached.set(a, [...(reached.get(a) ?? []), chain]);
    }
    for (const [a, found] of reached) {
      const [nearest, ...rest] = found.sort((p, q) => p.length - q.length || p.at(-1).localeCompare(q.at(-1)));
      const more = rest.length ? ` It reaches ${rest.map((c) => c.at(-1)).join(", ")} too.` : "";
      const shown = nearest.map((name, i) => (i === 1 && forTests.get(a).has(name) && !links.get(a).has(name) ? `${name} (${forTests.get(a).get(name)})` : name));
      problems.push(`${all.get(a).manifest}: ${a} reaches ${nearest.at(-1)} (${shown.join(" -> ")}), which ${label} forbids: ${rule.why}${more}`);
    }
  }

  // ------------------------------------------------------------ wrappers
  const wrapped = [];
  for (const w of config.wrappers ?? []) {
    if (!w.why?.trim()) problems.push(`layers.json's wrappers of ${w.crate} do not say why`);
    const may = new Set(w.wrappers ?? []);
    for (const n of may) if (!listed(n)) problems.push(`layers.json lets ${n} depend on ${w.crate}, but does not list ${n}`);
    const users = new Set();
    for (const m of all.values()) {
      for (const d of m.deps) {
        if (d.member || d.kind !== "normal" || d.name !== w.crate) continue;
        users.add(m.name);
        if (!may.has(m.name)) problems.push(`${m.manifest}: ${m.name}'s ${d.as}: only ${[...may].join(", ")} may depend on ${w.crate}, so go through ${may.size === 1 ? "it" : "one of them"}. ${w.why}`);
      }
    }
    for (const n of may) {
      if (all.has(n) && !users.has(n)) problems.push(`layers.json lets ${n} depend on ${w.crate}, which it no longer does: drop it`);
    }
    wrapped.push({ crate: w.crate, wrappers: [...may].filter((n) => all.has(n)) });
  }

  // ------------------------------------------------------------ the numbers
  const dependents = new Map();
  for (const out of links.values()) for (const to of out) dependents.set(to, (dependents.get(to) ?? 0) + 1);
  const stats = {
    members: all.size,
    outside: [...all.keys()].filter((n) => outside.has(n)),
    waiting,
    dependencies: [...links.values()].reduce((n, out) => n + out.size, 0),
    forbidden: { rules: rules.length, waiting: rules.filter((r) => !r.live).map((r) => r.label) },
    wrappers: wrapped,
    cycle: cycles([...all.keys()], withTests)[0] ?? [],
    middle: [...placed.values()]
      .filter((c) => c.middle && all.has(c.name))
      .map((c) => ({ crate: c.name, code: sizes.get(c.name) ?? 0, dependents: dependents.get(c.name) ?? 0 })),
    sourceMatches,
  };
  return { problems, stats };
}

function cargoMetadata(serverDir) {
  const cargo = process.env.CARGO || "cargo";
  const run = spawnSync(cargo, ["metadata", "--format-version", "1", "--no-deps", "--manifest-path", join(serverDir, "Cargo.toml")], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  if (run.error) throw new Error(`cannot run ${cargo}: ${run.error.message}`);
  if (run.status !== 0) throw new Error(run.stderr.trim() || `${cargo} metadata exited ${run.status}`);
  return JSON.parse(run.stdout);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const serverDir = resolve(dirname(fileURLToPath(import.meta.url)), "..", "server-rs");
  const config = JSON.parse(readFileSync(join(serverDir, "layers.json"), "utf8"));
  const workspace = workspaceCrates(serverDir);
  const sizes = new Map(workspace.map((w) => [w.name, w.lines]));
  const { stats: moduleStats } = checkLayers(moduleGraph(serverDir, { crates: workspace.map((w) => w.name) }), config, workspace);
  let meta;
  try {
    meta = cargoMetadata(serverDir);
  } catch (error) {
    console.error(`crate layers: cargo metadata cannot read server-rs/Cargo.toml:\n  ${error.message.split("\n").join("\n  ")}`);
    process.exit(1);
  }
  const { problems, stats } = checkCrates(meta, config, { sizes, sourceMatches: moduleStats.sourceMatches });
  if (process.argv.includes("--json")) {
    console.log(JSON.stringify({ ...stats, problems }, null, 2));
    process.exit(problems.length ? 1 : 0);
  }
  const summary = [
    `${stats.members} workspace members, ${stats.outside.length} of them outside the layers (${stats.outside.join(", ")}); declared, not members yet: ${stats.waiting.join(", ") || "none"}`,
    `${stats.dependencies} dependencies between them (dev- and build-dependencies aside)`,
    `forbidden: ${stats.forbidden.rules} rules, checked through every chain of dependencies, each crate's own dev- and build-dependencies included; waiting for their crates: ${stats.forbidden.waiting.join(", ") || "none"}`,
    `wrappers: ${stats.wrappers.map((w) => `${w.crate} only in ${w.wrappers.join(", ")}`).join("; ")}`,
    `largest crate cycle, dev-dependencies included: ${stats.cycle.length} crates${stats.cycle.length ? ` (${stats.cycle.join(", ")})` : ""}`,
    `middle crates (code lines, members depending on each): ${stats.middle.map((c) => `${c.crate} ${c.code} / ${c.dependents}`).join(", ")}`,
    `Source::Claude|Codex|Acp outside the provider crates: ${stats.sourceMatches}`,
  ];
  if (problems.length) {
    console.error(`crate layers: ${problems.length} problem(s)`);
    for (const p of problems) console.error(`  - ${p}`);
    console.error(
      "\nA crate depends on the crates its mayUse lists, and on the crates of the open layers below it " +
        "(server-rs/layers.json); nothing depends on the entry. Move the shared piece down to where both sides can reach it, " +
        "or declare a port in the lower crate that the upper one implements.",
    );
    process.exit(1);
  }
  console.log("crate layers: ok");
  for (const line of summary) console.log(`  ${line}`);
}
