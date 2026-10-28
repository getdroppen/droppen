// Tests for scripts/check-module-layers.mjs: `node --test scripts/check-module-layers.test.mjs`.
//
// The code reader, the graph over a small made-up crate, and the layer rules
// over made-up graphs. The real thing is `pnpm check:layers`.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { builtCode, checkLayers, cycles, moduleGraph, namedModules } from "./check-module-layers.mjs";

const named = (src) => namedModules(builtCode(src), "crate").map((u) => u.name);

describe("what counts as a use", () => {
  test("paths in code count; comments and strings do not", () => {
    const src = 'use crate::a::X; // crate::b\n/* crate::c /* nested */ crate::d */\nlet s = "crate::e"; let r = r#"crate::f "{" "#;\ncrate::g::h();\n';
    assert.deepEqual(named(src), ["a", "g"]);
  });

  test("grouped imports name each top-level module", () => {
    assert.deepEqual(named("use crate::{a::{B, C}, d, r#e::F};"), ["a", "d", "e"]);
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
  write("src/high/mod.rs", "use crate::low;\npub struct H;\n");
  write("src/only_tests.rs", "use crate::high::H;\n");
  write("src/main.rs", "mod cli;\nfn main() { the_server::low::f(); }\n");
  write("src/cli.rs", "use the_server::high::H;\nuse crate::main_only;\n");

  test("modules, their files and their edges; test files are not read", () => {
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
});

describe("the layer rules", () => {
  const graph = (edges) => ({
    modules: [...new Set(edges.flat())].sort(),
    files: [],
    problems: [],
    edges: new Map(
      [...new Set(edges.flat())].map((m) => [m, new Map(edges.filter(([a]) => a === m).map(([, b]) => [b, { count: 1, where: `src/${m}.rs:1` }]))]),
    ),
  });
  const layers = [
    { name: "base", modules: ["time"] },
    { name: "domain", modules: ["sessions", "inject"] },
    { name: "entry", modules: ["api"] },
  ];

  test("down and sideways is fine", () => {
    const { problems, stats } = checkLayers(graph([["sessions", "time"], ["inject", "sessions"], ["api", "inject"]]), { layers, allowed: [] });
    assert.deepEqual(problems, []);
    assert.equal(stats.upward, 0);
  });

  test("an upward use fails unless allowed lists it", () => {
    const g = graph([["sessions", "api"], ["api", "sessions"], ["inject", "time"]]);
    assert.match(checkLayers(g, { layers, allowed: [] }).problems.join("\n"), /sessions -> api: domain uses entry/);
    assert.deepEqual(checkLayers(g, { layers, allowed: [{ why: "test", edges: ["sessions -> api"] }] }).problems, []);
  });

  test("an allowed edge that is gone fails, so the list only shrinks", () => {
    const { problems } = checkLayers(graph([["sessions", "time"]]), { layers: [layers[0], { name: "domain", modules: ["sessions"] }], allowed: [{ why: "test", edges: ["sessions -> api"] }] });
    assert.match(problems.join("\n"), /allowed lists sessions -> api, which is no longer an upward use/);
  });

  test("every module has exactly one layer", () => {
    const { problems } = checkLayers(graph([["sessions", "time"], ["fresh", "time"]]), {
      layers: [{ name: "base", modules: ["time", "gone"] }, { name: "domain", modules: ["sessions", "time"] }],
      allowed: [],
    });
    const text = problems.join("\n");
    assert.match(text, /fresh: not in layers.json/);
    assert.match(text, /places gone, which is no longer a module/);
    assert.match(text, /places time twice/);
  });

  test("the numbers: the largest cycle and who is used most", () => {
    const { stats } = checkLayers(graph([["sessions", "inject"], ["inject", "sessions"], ["api", "sessions"], ["inject", "time"]]), { layers, allowed: [] });
    assert.deepEqual(stats.cycle, ["inject", "sessions"]);
    assert.deepEqual(Object.entries(stats.fanIn)[0], ["sessions", 2]);
  });
});
