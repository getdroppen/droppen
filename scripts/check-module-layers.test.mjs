// Tests for scripts/check-module-layers.mjs: `node --test scripts/check-module-layers.test.mjs`.
//
// The code reader, the graph over a small made-up crate, and the crate rules
// over made-up graphs. The real thing is `pnpm check:layers`.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { builtCode, checkLayers, cycles, moduleGraph, namedModules, pathsFrom } from "./check-module-layers.mjs";

const named = (src) => namedModules(builtCode(src), "crate").map((u) => u.name);

describe("what counts as a use", () => {
  test("paths in code count; comments and strings do not", () => {
    const src = 'use crate::a::X; // crate::b\n/* crate::c /* nested */ crate::d */\nlet s = "crate::e"; let r = r#"crate::f "{" "#;\ncrate::g::h();\n';
    assert.deepEqual(named(src), ["a", "g"]);
  });

  test("grouped imports name each top-level module", () => {
    assert.deepEqual(named("use crate::{a::{B, C}, d, r#e::F};"), ["a", "a", "d", "e"]);
  });

  test("a path is its segments, groups expanded, `self` and globs ending it", () => {
    const paths = pathsFrom(builtCode("use crate::a::{b::{C, self}, d::*}; crate::e::F::<T>::new();"), "crate").map((p) => p.path.join("::"));
    assert.deepEqual(paths, ["a::b::C", "a::b", "a::d", "e::F"]);
  });

  test("a test-only item does not count, whatever its shape; cfg(not(test)) does", () => {
    const src = [
      "struct S { x: u32,",
      "  #[cfg(test)]",
      "  hook: Option<HashMap<u8, Box<dyn Fn(u8, u8)>>>,",
      "  y: crate::a::Y }",
      "#[cfg(test)]",
      "mod tests { use crate::b; fn f() { let s = \"}\"; let c = '{'; } }",
      "#[cfg(all(test, unix))]",
      "use crate::c;",
      "#[cfg(not(test))]",
      "use crate::d;",
      "fn g<'a>(s: &'a str) -> &'a str { crate::e::f(); s }",
    ].join("\n");
    assert.deepEqual(named(src), ["a", "d", "e"]);
  });

  test("pub(in …) limits who may use an item; it is not a use", () => {
    assert.deepEqual(named("pub(in crate::a) fn f() { crate::b::g() }"), ["b"]);
  });

  test("lines still count after stripping", () => {
    const src = "// one\n#[cfg(test)]\nmod tests {\n}\nuse crate::a;\n";
    assert.equal(builtCode(src).split("\n").length, src.split("\n").length);
  });
});

describe("the module graph of a crate", () => {
  const root = mkdtempSync(join(tmpdir(), "module-layers-"));
  after(() => rmSync(root, { recursive: true, force: true }));
  const write = (path, text) => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  };
  write("Cargo.toml", '[package]\nname = "the-server"\n');
  write("src/lib.rs", "pub mod low;\npub mod high;\n#[cfg(test)]\nmod only_tests;\n");
  write("src/low.rs", "pub fn f() {}\nmod inner;\n#[cfg(test)]\nmod tests;\n");
  write("src/low/inner.rs", "use crate::high::H;\n");
  write("src/low/tests.rs", "use crate::high::Other;\n");
  write("src/high/mod.rs", 'use crate::low;\npub struct H;\nmod part;\npub fn g() { part::f(); let s = "Source::Codex"; }\n');
  write("src/high/part.rs", "use super::H;\nuse super::super::low::f as lf;\npub fn f() { match s { Source::Claude => 1, Source::Acp => 2 } }\n");
  write("src/only_tests.rs", "use crate::high::H;\n");
  write("src/main.rs", "mod cli;\nfn main() { the_server::low::f(); }\n");
  write("src/cli.rs", "use the_server::high::H;\nuse crate::main_only;\nuse droppen_base::low::f;\n");

  test("modules, their files and their edges; test files and other crates' paths are not read", () => {
    const g = moduleGraph(root);
    assert.deepEqual(g.modules, ["cli", "high", "low", "main"]);
    assert.ok(g.files.includes("src/low/inner.rs") && !g.files.includes("src/low/tests.rs") && !g.files.includes("src/only_tests.rs"));
    assert.deepEqual([...g.edges.get("low").keys()], ["high"]);
    assert.deepEqual([...g.edges.get("high").keys()], ["low"]);
    assert.deepEqual([...g.edges.get("main").keys()], ["low"]);
    assert.deepEqual([...g.edges.get("cli").keys()], ["high"]);
    assert.deepEqual(g.problems, []);
    assert.deepEqual(cycles(g.modules, g.edges), [["high", "low"]]);
  });

  test("a child module's name, self:: and super:: are uses, resolved to module paths", () => {
    const g = moduleGraph(root);
    assert.ok(g.paths.has("high::part") && g.paths.has("low::inner"));
    const uses = g.uses.filter((u) => u.from[0] === "high").map((u) => `${u.from.join("::")} -> ${u.to.join("::")}`);
    assert.deepEqual(uses.sort(), ["high -> high::part::f", "high -> low", "high::part -> high::H", "high::part -> low::f"]);
  });

  test("each file's size and Source matches, strings and comments aside", () => {
    const g = moduleGraph(root);
    const part = g.sizes.find((s) => s.file === "src/high/part.rs");
    assert.equal(part.lines, 3);
    assert.deepEqual(part.sources, ["src/high/part.rs:3", "src/high/part.rs:3"]);
    assert.deepEqual(g.sizes.find((s) => s.file === "src/high/mod.rs").sources, []);
  });
});

describe("the crate rules", () => {
  // `uses` as ["from -> to", …] between module paths, each `to` maybe
  // running on into an item; `more` are modules nothing here uses.
  const graph = (uses, sources = {}, more = []) => {
    const pairs = uses.map((u) => u.split(" -> ").map((p) => p.split("::")));
    const paths = new Set([...more, ...pairs.flatMap(([a, b]) => [a.join("::"), b.slice(0, 2).join("::"), a[0], b[0]])]);
    const modules = [...new Set([...paths].map((p) => p.split("::")[0]))].sort();
    const edges = new Map(modules.map((m) => [m, new Map()]));
    for (const [a, b] of pairs) if (a[0] !== b[0]) edges.get(a[0]).set(b[0], { count: 1, where: `src/${a[0]}.rs:1` });
    return {
      modules,
      paths,
      files: [],
      problems: [],
      edges,
      uses: pairs.map(([from, to]) => ({ from, to, where: `src/${from.join("/")}.rs:1` })),
      sizes: Object.entries(sources).map(([path, n]) => ({ file: `src/${path}.rs`, path: path.split("::"), lines: 10, sources: Array(n).fill(`src/${path}.rs:1`) })),
    };
  };
  const layers = [
    { name: "base", open: true, crates: [{ name: "c-time", modules: ["time"] }] },
    {
      name: "platform",
      open: true,
      crates: [
        { name: "c-paths", modules: ["paths"] },
        { name: "c-http", modules: ["http"], mayUse: ["c-paths"] },
      ],
    },
    {
      name: "domain",
      middle: true,
      crates: [
        { name: "c-sessions", modules: ["sessions"] },
        { name: "c-accounts", modules: ["accounts"], mayUse: ["c-sessions"] },
        { name: "c-notify", modules: ["notify", "push"], mayUse: ["c-sessions", "c-accounts"] },
      ],
    },
    {
      name: "providers",
      crates: [
        { name: "c-provider", modules: ["inject"], mayUse: ["c-sessions", "c-accounts"] },
        { name: "c-claude", modules: ["inject::resident"], mayUse: ["c-provider", "c-sessions"] },
      ],
    },
    { name: "entry", crates: [{ name: "c-entry", modules: ["api"], mayUse: "*" }] },
  ];
  const placed = layers.flatMap((l) => l.crates.flatMap((c) => c.modules));
  const check = (uses, extra = {}) => checkLayers(graph(uses, extra.sources, placed), { layers, allowed: [], ...extra });

  test("what mayUse lists, and the open layers below, is fine", () => {
    const { problems, stats } = check([
      "sessions -> time",
      "http -> paths",
      "sessions -> http",
      "accounts -> sessions",
      "notify -> accounts",
      "push -> sessions",
      "inject -> accounts",
      "inject::resident -> inject",
      "api -> inject::resident",
      "api -> push",
    ]);
    assert.deepEqual(problems, []);
    assert.equal(stats.violations, 0);
  });

  test("anything else fails unless allowed lists it: sideways, up, or inside an open layer", () => {
    const uses = ["paths -> http", "sessions -> accounts", "accounts -> inject", "inject -> api"];
    const text = check(uses).problems.join("\n");
    assert.match(text, /paths -> http: c-paths may not use c-http: c-paths's mayUse does not list c-http/);
    assert.match(text, /sessions -> accounts: c-sessions may not use c-accounts: c-sessions's mayUse does not list c-accounts/);
    assert.match(text, /accounts -> inject: c-accounts may not use c-provider: c-provider is in a higher layer \(providers\)/);
    assert.match(text, /inject -> api: c-provider may not use c-entry/);
    assert.deepEqual(check(uses, { allowed: [{ why: "test", edges: uses }] }).problems, []);
  });

  test("a lower layer that is not open is used only where mayUse lists it", () => {
    const { problems } = check(["inject -> push::notify_rotation"]);
    assert.deepEqual(problems, ["inject -> push: c-provider may not use c-notify: c-provider's mayUse does not list c-notify (1 use(s), the first at src/inject.rs:1)"]);
  });

  test("what a crate may use does not pass to the crates that may use it", () => {
    const { problems } = check(["inject::resident -> inject", "inject -> accounts", "inject::resident -> accounts::limits::clip"]);
    assert.deepEqual(problems, [
      "inject::resident -> accounts: c-claude may not use c-accounts: c-claude's mayUse does not list c-accounts (1 use(s), the first at src/inject/resident.rs:1)",
    ]);
  });

  test("a module placed apart from its parent goes with its own crate, its items too", () => {
    const { problems } = check(["inject::resident -> inject", "api -> inject::resident::Driver", "inject -> inject::resident::Driver"]);
    assert.deepEqual(problems, ["inject -> inject::resident: c-provider may not use c-claude: c-provider's mayUse does not list c-claude (1 use(s), the first at src/inject.rs:1)"]);
  });

  test("a new crate::push use in a provider crate's code fails, read from the source", () => {
    const root = mkdtempSync(join(tmpdir(), "module-layers-push-"));
    after(() => rmSync(root, { recursive: true, force: true }));
    const write = (path, text) => {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), text);
    };
    write("Cargo.toml", '[package]\nname = "the-server"\n');
    write("src/lib.rs", "pub mod inject;\npub mod push;\npub mod sessions;\n");
    write("src/sessions.rs", "pub struct Summary;\n");
    write("src/push.rs", "use crate::sessions::Summary;\npub fn notify_turn(_: &Summary) {}\n");
    write("src/inject/mod.rs", "use crate::sessions::Summary;\n\npub fn finish(s: &Summary) {\n    crate::push::notify_turn(s);\n}\n");
    const config = {
      layers: [
        { name: "domain", crates: [{ name: "c-sessions", modules: ["sessions"] }, { name: "c-notify", modules: ["push"], mayUse: ["c-sessions"] }] },
        { name: "providers", crates: [{ name: "c-provider", modules: ["inject"], mayUse: ["c-sessions"] }] },
      ],
      allowed: [],
    };
    assert.deepEqual(checkLayers(moduleGraph(root), config).problems, [
      "inject -> push: c-provider may not use c-notify: c-provider's mayUse does not list c-notify (1 use(s), the first at src/inject/mod.rs:4)",
    ]);
  });

  test("an allowed edge that is gone fails, so the list only shrinks", () => {
    const { problems } = check(["sessions -> time"], { allowed: [{ why: "test", edges: ["sessions -> api"] }] });
    assert.match(problems.join("\n"), /allowed lists sessions -> api, which is no longer a use between crates that may not use each other/);
  });

  test("every module has exactly one crate, and mayUse names crates a crate could use", () => {
    const { problems } = checkLayers(graph(["sessions -> time", "fresh -> time"]), {
      layers: [
        { name: "base", open: true, crates: [{ name: "a", modules: ["time", "gone"] }] },
        { name: "domain", crates: [{ name: "b", modules: ["sessions", "time"], mayUse: ["a", "nowhere", "c"] }] },
        { name: "entry", crates: [{ name: "c", modules: [] }] },
      ],
      allowed: [],
    });
    const text = problems.join("\n");
    assert.match(text, /fresh: not in layers.json/);
    assert.match(text, /places gone, which is no longer a module/);
    assert.match(text, /places time twice/);
    assert.match(text, /b lists a in mayUse: every crate above its layer, base, may use it already/);
    assert.match(text, /b may use nowhere, which layers.json does not list/);
    assert.match(text, /b lists c in mayUse: a crate of a higher layer/);
  });

  test("Source matches outside the provider crates only go down", () => {
    const uses = ["sessions -> time", "inject -> sessions"];
    const sources = { sessions: 2, "inject::resident": 5, inject: 1 };
    const at = (count) => check(uses, { sources, sourceMatches: { providers: ["c-provider", "c-claude"], count } });
    assert.deepEqual(at(2).problems, []);
    assert.equal(at(2).stats.sourceMatches, 2);
    assert.match(at(1).problems.join("\n"), /2 matches of Source::Claude\|Codex\|Acp outside the provider crates, more than the 1/);
    assert.match(at(3).problems.join("\n"), /fewer than the 3 sourceMatches records: lower it to 2/);
  });

  test("the numbers: the largest cycles, the middle crates, who is used most", () => {
    const { stats } = check(["sessions -> accounts", "accounts -> sessions", "notify -> sessions", "api -> sessions", "accounts -> time"], { sources: { sessions: 0, accounts: 0 } });
    assert.deepEqual(stats.cycle, ["accounts", "sessions"]);
    assert.deepEqual(stats.crateCycle, ["c-accounts", "c-sessions"]);
    assert.deepEqual(stats.middle, [
      { crate: "c-sessions", code: 10, fanIn: 3 },
      { crate: "c-accounts", code: 10, fanIn: 1 },
      { crate: "c-notify", code: 0, fanIn: 0 },
    ]);
    assert.deepEqual(Object.entries(stats.fanIn)[0], ["sessions", 3]);
  });
});
