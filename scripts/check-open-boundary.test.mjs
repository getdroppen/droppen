// Tests for scripts/check-open-boundary.mjs: `node --test scripts/check-open-boundary.test.mjs`.
//
// No cargo: the comment stripper, the source and manifest rules, what is read
// out of `cargo metadata`, and the whole check over small made-up trees, with
// a made-up `cargo metadata`. The real thing is `pnpm check:boundary`.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { checkOpenBoundary, codeOnly, isHub, isTheHub, manifestMentions, metadataProblems, sourceProblems } from "./check-open-boundary.mjs";

describe("comments out, strings kept", () => {
  test("Rust: line, doc and nested block comments go; lines still count", () => {
    const src = '//! rh_hub::x\nlet a = "// kept"; /* outer /* inner */ still */ let b = 1;\n/// RH_HUB_X\n';
    const code = codeOnly(src, "rs");
    assert.equal(code.split("\n").length, src.split("\n").length);
    assert.ok(!code.includes("rh_hub") && !code.includes("RH_HUB_X") && !code.includes("still"));
    assert.ok(code.includes('"// kept"') && code.includes("let b = 1;"));
  });

  test("Rust: raw strings, char literals and lifetimes", () => {
    const code = codeOnly(`let s = r#"a " // b"#; let q = '"'; fn f<'a>(x: &'a str) {} // gone`, "rs");
    assert.ok(code.includes(`r#"a " // b"#`));
    assert.ok(code.includes(`'"'`) && code.includes("&'a str"));
    assert.ok(!code.includes("gone"));
  });

  test("TypeScript: regexes, templates and JSX prose", () => {
    const src = [
      "const re = /\\/\\/ not a comment/; // first gone",
      "const t = `a ${f({ x: `b ${'//'}` })} // in text`; /* c */",
      "const p = <p>Don't stop</p>; const d = a / b / c; // after division",
    ].join("\n");
    const code = codeOnly(src, "js");
    assert.ok(code.includes("/\\/\\/ not a comment/"));
    assert.ok(!code.includes("first gone") && !code.includes("/* c */") && !code.includes("after division"));
    assert.ok(code.includes("// in text`"));
    assert.ok(code.includes("const d = a / b / c;"));
  });
});

describe("sources", () => {
  test("the hub crate in code, not in comments", () => {
    assert.deepEqual(sourceProblems("// see rh_hub::api\nfn f() {}", "rs", "a.rs"), []);
    const found = sourceProblems("use rh_hub::api;\nlet x = droppen_hub::Y;", "rs", "a.rs");
    assert.equal(found.length, 2);
    assert.match(found[0], /^a\.rs:1: `rh_hub`/);
    assert.match(found[1], /^a\.rs:2: `droppen_hub`/);
    // a name that only contains it is another name
    assert.deepEqual(sourceProblems("let my_rh_hub_x = 1;", "rs", "a.rs"), []);
  });

  test("a path into the hub's source", () => {
    const found = sourceProblems('import plans from "../../server-rs/crates/rh-hub/src/plans.rs?raw";', "js", "web/src/a.ts");
    assert.equal(found.length, 1);
    assert.match(found[0], /^web\/src\/a\.ts:1: `rh-hub\/` — a path into the hub's source/);
    assert.deepEqual(sourceProblems("/** mirrors server-rs/crates/rh-hub/src/plans.rs */\nexport const x = 1;", "js", "web/src/a.ts"), []);
    // the hub's page marker is a documented name, not a path
    assert.deepEqual(sourceProblems(`document.querySelector('meta[name="rh-hub"]');`, "js", "web/src/a.ts"), []);
    assert.deepEqual(sourceProblems(`const shell = Buffer.from('<head><meta name="rh-hub" content="wss://x" /></head>');`, "js", "scripts/a.mjs"), []);
  });

  test("a path into the hub's side from next door, however it is spelled", () => {
    const forms = [
      'const PLANS: &str = include_str!("../../rh-hub/src/plans.rs");',
      '#[path = "../../rh-hub/src/plans.rs"]\nmod plans;',
      'let data = format!("{}/../rh-hub/data", env!("CARGO_MANIFEST_DIR"));',
      'let e2e = "../rh-hub-tools/src/bin/rh-cloud-e2e.rs";',
      'let hub = Path::new(root).join("crates/rh-hub");',
      // the hub's directory itself, with nothing after it
      'let hub = concat!(env!("CARGO_MANIFEST_DIR"), "/../rh-hub");',
      'let hub = Path::new(root).join("rh-hub");',
      'let mut p = PathBuf::from(root); p.push("rh-hub-tools");',
    ];
    for (const src of forms) {
      const found = sourceProblems(src, "rs", "server-rs/crates/rh-devkit/src/a.rs");
      assert.equal(found.length, 1, src);
      assert.match(found[0], /a path into the hub's source/, src);
    }
    for (const src of ['const hub = join(root, "server-rs", "crates", "rh-hub");', 'const hub = new URL("../rh-hub", import.meta.url);']) {
      const found = sourceProblems(src, "js", "scripts/a.mjs");
      assert.equal(found.length, 1, src);
      assert.match(found[0], /a path into the hub's source/, src);
    }
    // a file or a binary named after it is not a path into it
    for (const src of ['let bin = "target/debug/rh-hub-stress";', '#[path = "bin/rh-hub-stress.rs"]\nmod stress;', 'let s = "rh-hub-web";']) {
      assert.deepEqual(sourceProblems(src, "rs", "server-rs/crates/rh-devkit/src/a.rs"), [], src);
    }
    // nor is the documented route of which hosts a hub serves
    assert.deepEqual(sourceProblems('pub const DISCOVERY_PATH: &str = "/.well-known/droppen-hub";', "rs", "server-rs/src/a.rs"), []);
    assert.deepEqual(sourceProblems('await fetch("/.well-known/droppen-hub");', "js", "web/src/a.ts"), []);
    assert.equal(sourceProblems('let p = "/srv/well-known/droppen-hub";', "rs", "server-rs/src/a.rs").length, 1, "only that route");
  });

  test("shell and YAML: `#` comments out, quotes and `$#` do not", () => {
    const sh = [
      "# DROPPEN_HUB_IN_A_COMMENT=1",
      'echo "# not a comment: DROPPEN_HUB_BIND"', // a string
      "n=${#items[@]} DROPPEN_HUB_ORIGIN=x # DROPPEN_HUB_AFTER_A_COMMENT",
      "cat ../rh-hub/Cargo.toml",
    ].join("\n");
    const found = sourceProblems(sh, "sh", "server-rs/crates/rh-devkit/scripts/a.sh").map((p) => /`([^`]+)`/.exec(p)[1]);
    assert.deepEqual(found, ["rh-hub/", "DROPPEN_HUB_BIND", "DROPPEN_HUB_ORIGIN"]);
    // a here-document is what a command reads: its `#` lines are not comments
    const heredoc = [
      "cat >run.sh <<'EOF'",
      "# DROPPEN_HUB_IN_A_HEREDOC=1",
      "EOF",
      "# DROPPEN_HUB_IN_A_COMMENT_AFTER=1",
      "cat <<-END",
      "\t# DROPPEN_HUB_IN_AN_INDENTED_ONE=1",
      "\tEND",
      "x=1 # DROPPEN_HUB_NOT_EITHER",
    ].join("\n");
    assert.deepEqual(
      sourceProblems(heredoc, "sh", "scripts/a.sh").map((p) => /`([^`]+)`/.exec(p)[1]),
      ["DROPPEN_HUB_IN_A_HEREDOC", "DROPPEN_HUB_IN_AN_INDENTED_ONE"],
    );
    const yml = ["name: Don't build the hub # DROPPEN_HUB_NOT", "env:", "  RH_HUB_TEST_X: '1' # RH_HUB_NOT"].join("\n");
    assert.deepEqual(
      sourceProblems(yml, "sh", ".github/workflows/x.yml").map((p) => /`([^`]+)`/.exec(p)[1]),
      ["RH_HUB_TEST_X"],
    );
  });

  test("the hub's environment, but for the open side's own variables", () => {
    const found = sourceProblems('let o = std::env::var("DROPPEN_HUB_ORIGIN");\nlet p = format!("RH_HUB_{x}");', "rs", "server-rs/src/a.rs");
    assert.equal(found.length, 2);
    assert.match(found[0], /`DROPPEN_HUB_ORIGIN` — the hub's environment/);
    assert.match(found[1], /`RH_HUB_` —/);
    for (const name of ["RH_HUB_DEFAULT", "DROPPEN_HUB_DEFAULT", "RH_HUB_API_TRAFFIC", "DROPPEN_HUB_API_TRAFFIC"]) {
      assert.deepEqual(sourceProblems(`std::env::var("${name}")`, "rs", "server-rs/src/a.rs"), [], name);
    }
    assert.deepEqual(sourceProblems("/** off unless `RH_HUB_CHANNEL_RELAY` */\nexport {};", "js", "web/src/a.ts"), []);
  });

  test("an allowed file names only what it is allowed to", () => {
    assert.deepEqual(sourceProblems('old_name("DROPPEN_HUB_ORIGIN")', "rs", "server-rs/crates/rh-relay/src/config.rs"), []);
    assert.equal(sourceProblems('old_name("DROPPEN_HUB_BIND")', "rs", "server-rs/crates/rh-relay/src/config.rs").length, 1);
    assert.deepEqual(sourceProblems('DROPPEN_HUB_BIND=127.0.0.1 "$hub" serve', "sh", "server-rs/crates/rh-devkit/scripts/cloud-dev.sh"), []);
    assert.equal(sourceProblems('DROPPEN_HUB_NOT_DOCUMENTED=1 "$hub" serve', "sh", "server-rs/crates/rh-devkit/scripts/cloud-dev.sh").length, 1);
    // the devkit's e2e harness went to the hub's side (rh-hub-tools): no allowance is left for it here
    assert.equal(sourceProblems('("DROPPEN_HUB_BIND", "127.0.0.1")', "rs", "server-rs/crates/rh-devkit/src/bin/rh-cloud-e2e.rs").length, 1);
  });
});

describe("manifests", () => {
  test("a dependency or a member that names the hub's side", () => {
    const toml = [
      "# the hub (crates/rh-hub) is closed", // a comment
      'members = [".", "crates/rh-proto", "crates/rh-hub"]',
      "[[bin]]",
      'name = "rh-hub-stress"', // a binary (once the devkit's), not a package
      'path = "src/bin/rh-hub-stress.rs"',
      "[dev-dependencies]",
      'rh-hub = { path = "../rh-hub" }',
      "[build-dependencies.droppen-hub]",
      'description = "a # in a string rh-hub"',
      "[dependencies]",
      'rh-hub-tools = { path = "../rh-hub-tools" }',
      'include = ["src/bin/rh-hub-stress.rs"]', // a file name
      'members = ["crates/rh-hub-tools"]',
    ].join("\n");
    assert.deepEqual(
      manifestMentions(toml).map((m) => m.line),
      [2, 7, 8, 9, 11, 13],
    );
  });
  test("names", () => {
    assert.ok(isHub("rh-hub") && isHub("droppen-hub") && isHub("rh_hub"));
    assert.ok(isHub("rh-hub-tools") && isHub("droppen-hub-tools") && isHub("rh_hub_tools"));
    assert.ok(isTheHub("rh-hub") && isTheHub("droppen_hub") && !isTheHub("rh-hub-tools"));
    assert.ok(!isHub("rh-proto") && !isHub("rh-hubble") && !isHub("my-rh-hub"));
  });
});

describe("cargo metadata", () => {
  const root = mkdtempSync(join(tmpdir(), "open-boundary-"));
  after(() => rmSync(root, { recursive: true, force: true }));
  const at = (...p) => join(root, ...p);
  const pkg = (name, dir, dependencies) => ({ name, source: null, manifest_path: at(...dir, "Cargo.toml"), dependencies });
  const dep = (name, more = {}) => ({ name, kind: null, rename: null, target: null, ...more });

  test("an open tree passes", () => {
    const meta = {
      packages: [
        pkg("remote-hand-server", ["server-rs"], [dep("rh-proto", { path: at("server-rs", "crates", "rh-proto") }), dep("serde", { source: "registry+https://github.com/rust-lang/crates.io-index" })]),
        pkg("rh-proto", ["server-rs", "crates", "rh-proto"], []),
        // the hub may depend on the open crates, and on itself
        pkg("rh-hub", ["server-rs", "crates", "rh-hub"], [dep("rh-proto", { path: at("server-rs", "crates", "rh-proto") }), dep("rh-hub", { kind: "dev", path: at("server-rs", "crates", "rh-hub") })]),
      ],
    };
    assert.deepEqual(metadataProblems(meta, root), []);
  });

  test("a dependency of any kind on the hub, from any source", () => {
    const meta = {
      packages: [
        pkg("rh-devkit", ["server-rs", "crates", "rh-devkit"], [dep("rh-hub", { kind: "dev", path: at("server-rs", "crates", "rh-hub") })]),
        pkg("rh-relay", ["server-rs", "crates", "rh-relay"], [dep("droppen-hub", { kind: "build", source: "git+https://example.invalid/hub" })]),
        pkg("rh-client", ["server-rs", "crates", "rh-client"], [dep("rh-hub", { rename: "hub" })]),
      ],
    };
    const found = metadataProblems(meta, root);
    assert.equal(found.length, 3);
    assert.match(found[0], /^server-rs\/crates\/rh-devkit\/Cargo\.toml: rh-devkit has a dev dependency rh-hub/);
    assert.match(found[1], /rh-relay has a build dependency droppen-hub/);
    assert.match(found[2], /rh-client has a normal dependency hub \(rh-hub\)/);
  });

  test("a path outside the repository", () => {
    const outside = join(root, "..", "elsewhere", "foo");
    const meta = {
      packages: [
        pkg("rh-devkit", ["server-rs", "crates", "rh-devkit"], [dep("foo", { path: outside })]),
        { name: "stray", source: null, manifest_path: join(outside, "Cargo.toml"), dependencies: [] },
        // the hub is not held to it
        pkg("rh-hub", ["server-rs", "crates", "rh-hub"], [dep("foo", { path: outside })]),
      ],
    };
    const found = metadataProblems(meta, root);
    assert.equal(found.length, 2);
    assert.match(found[0], /rh-devkit's normal dependency foo is a path outside this repository/);
    assert.match(found[1], /stray is outside this repository/);
  });

  test("the hub's side: rh-hub-tools may depend on the hub, and nothing open on it", () => {
    const meta = {
      packages: [
        pkg("rh-hub", ["server-rs", "crates", "rh-hub"], [dep("rh-hub-tools", { kind: "dev", path: at("server-rs", "crates", "rh-hub-tools") })]),
        pkg("rh-hub-tools", ["server-rs", "crates", "rh-hub-tools"], [dep("rh-hub", { path: at("server-rs", "crates", "rh-hub") }), dep("rh-devkit", { path: at("server-rs", "crates", "rh-devkit") })]),
        pkg("rh-devkit", ["server-rs", "crates", "rh-devkit"], [dep("rh-hub-tools", { kind: "dev", path: at("server-rs", "crates", "rh-hub-tools") })]),
      ],
    };
    const found = metadataProblems(meta, root);
    assert.equal(found.length, 1);
    assert.match(found[0], /rh-devkit has a dev dependency rh-hub-tools/);
  });
});

describe("the whole check, over a made-up tree", () => {
  const trees = [];
  after(() => trees.forEach((t) => rmSync(t, { recursive: true, force: true })));

  /** A tree with `files` (path → text), and `cargo metadata` listing every `Cargo.toml`'s package with no dependencies. */
  function tree(files) {
    const root = mkdtempSync(join(tmpdir(), "open-boundary-tree-"));
    trees.push(root);
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), text);
    }
    const packages = Object.keys(files)
      .filter((p) => p.endsWith("Cargo.toml"))
      .map((p) => ({ name: /name = "([^"]+)"/.exec(files[p])[1], source: null, manifest_path: join(root, p), dependencies: [] }));
    return { root, metadata: () => ({ meta: { packages } }) };
  }
  const crate = (name) => `[package]\nname = "${name}"\n`;
  const open = {
    "server-rs/Cargo.toml": `${crate("remote-hand-server")}[workspace]\n`,
    "server-rs/src/main.rs": 'fn main() { let _ = std::env::var("DROPPEN_HUB_DEFAULT"); }\n',
    "server-rs/crates/rh-proto/Cargo.toml": crate("rh-proto"),
    "server-rs/crates/rh-proto/src/lib.rs": "pub fn f() {}\n",
  };
  // the hub's side may name the hub's environment: it is not scanned
  const tools = {
    "server-rs/crates/rh-hub-tools/Cargo.toml": crate("rh-hub-tools"),
    "server-rs/crates/rh-hub-tools/src/lib.rs": 'pub const K: &str = "DROPPEN_HUB_SESSION_KEY";\n',
  };
  const hub = {
    "server-rs/crates/rh-hub/Cargo.toml": crate("rh-hub"),
    "server-rs/crates/rh-hub/src/lib.rs": 'pub const K: &str = "DROPPEN_HUB_ORIGIN";\n',
  };

  test("with the hub and its side", () => {
    const { root, metadata } = tree({ ...open, ...hub, ...tools });
    const { problems, notes } = checkOpenBoundary(root, { metadata });
    assert.deepEqual(problems, []);
    assert.ok(notes.some((n) => n.includes("the hub's side is in this tree (rh-hub, rh-hub-tools)")), notes.join("\n"));
    assert.ok(notes.some((n) => n.startsWith("2 open packages")), notes.join("\n"));
  });

  test("the public shape", () => {
    const { root, metadata } = tree(open);
    const { problems, notes } = checkOpenBoundary(root, { metadata });
    assert.deepEqual(problems, []);
    assert.ok(notes.includes("the hub is not in this tree (the public repository's shape)"), notes.join("\n"));
  });

  test("the hub's side without the hub", () => {
    const { root, metadata } = tree({ ...open, ...tools });
    const { problems } = checkOpenBoundary(root, { metadata });
    assert.equal(problems.length, 1, problems.join("\n"));
    assert.match(problems[0], /rh-hub-tools.*on the hub's side, but the hub is not in this tree/);
  });

  test("the tests, the SDK, the open crates' scripts and the workflows are scanned too; the hub's side is not", () => {
    const { root, metadata } = tree({
      ...open,
      ...hub,
      ...tools,
      "server-rs/crates/rh-hub-tools/scripts/e2e.sh": "DROPPEN_HUB_BIND=127.0.0.1 ../rh-hub/run\n",
      "server-rs/crates/rh-proto/scripts/dev.sh": "DROPPEN_HUB_BIND=127.0.0.1 cargo run # DROPPEN_HUB_NOT\n",
      "web/test/plans.test.ts": 'const crate = new URL("../../server-rs/crates/rh-hub/", import.meta.url);\n',
      "packages/plugin-sdk/src/index.ts": 'export const hub = "DROPPEN_HUB_ORIGIN";\n',
      ".github/workflows/ci.yml": "env:\n  RH_HUB_TEST_REDIS_URL: redis://127.0.0.1\n",
      ".github/workflows/other.yml": "env:\n  RH_HUB_TEST_REDIS_URL: redis://127.0.0.1\n",
    });
    const { problems, notes } = checkOpenBoundary(root, { metadata });
    assert.deepEqual(problems.map((p) => p.replace(/ — .*/, "")), [
      "server-rs/crates/rh-proto/scripts/dev.sh:1: `DROPPEN_HUB_BIND`",
      "web/test/plans.test.ts:1: `rh-hub/`",
      "packages/plugin-sdk/src/index.ts:1: `DROPPEN_HUB_ORIGIN`",
      // ci.yml may name it (ALLOWED); another workflow may not
      ".github/workflows/other.yml:2: `RH_HUB_TEST_REDIS_URL`",
    ]);
    // what ALLOWED lets ci.yml name and it does not, and the files it lists that are not here
    assert.ok(notes.some((n) => n.startsWith("ALLOWED lets .github/workflows/ci.yml name rh-hub/, RH_HUB_TEST_DATABASE_URL, RH_HUB_TEST_REQUIRE_DB,")), notes.join("\n"));
    assert.ok(notes.some((n) => n.startsWith("ALLOWED lists server-rs/crates/rh-devkit/scripts/cloud-dev.sh, which is no longer here")), notes.join("\n"));
  });

  test("a hub variable in an open crate", () => {
    const { root, metadata } = tree({ ...open, ...hub, "server-rs/crates/rh-proto/src/lib.rs": 'pub const K: &str = "DROPPEN_HUB_ORIGIN";\n' });
    const { problems } = checkOpenBoundary(root, { metadata });
    assert.equal(problems.length, 1, problems.join("\n"));
    assert.match(problems[0], /^server-rs\/crates\/rh-proto\/src\/lib\.rs:1: `DROPPEN_HUB_ORIGIN`/);
  });
});
