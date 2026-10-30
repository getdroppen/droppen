// Tests for scripts/check-layers.mjs: `node --test scripts/check-layers.test.mjs`.
//
// The code reader, the crate rules over made-up workspaces, the Source
// ratchet, and server-rs/layers.json's own rules as the check reads them. The
// real thing is `pnpm check:layers`.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, describe, test } from "node:test";
import { fileURLToPath } from "node:url";
import { builtCode, checkCrates, crateCode } from "./check-layers.mjs";

const ROOT = "/ws";

/**
 * `cargo metadata` of a workspace whose members are `spec`'s keys, each with
 * its dependencies as a manifest would give them: `"b"`, `"dev b"`,
 * `"build b"`, `"alias = b"` (renamed), `"b for cfg(unix)"`, and
 * `"registry b"` for a crate from a registry that has a member's name. A
 * name that is no member is an external crate.
 */
function workspace(spec) {
  const dir = (name) => `${ROOT}/crates/${name}`;
  const packages = Object.entries(spec).map(([name, deps]) => ({
    id: `path+file://${dir(name)}#0.1.0`,
    name,
    manifest_path: `${dir(name)}/Cargo.toml`,
    dependencies: deps.map((text) => {
      const [, kind = null, registry, rename = null, dep, target = null] = /^(?:(dev|build) )?(?:(registry) )?(?:(\S+) = )?(\S+)(?: for (.+))?$/.exec(text);
      const member = !registry && dep in spec;
      return { name: dep, kind, rename, target, optional: false, path: member ? dir(dep) : null, source: member ? null : "registry+https://github.com/rust-lang/crates.io-index" };
    }),
  }));
  return { packages, workspace_members: packages.map((p) => p.id), workspace_root: ROOT };
}

const config = {
  layers: [
    { name: "base", open: true, crates: [{ name: "base" }] },
    { name: "platform", open: true, middle: true, crates: [{ name: "pty", mayUse: [] }, { name: "ws-client", mayUse: [] }] },
    {
      name: "domain",
      middle: true,
      crates: [
        { name: "auth", mayUse: [] },
        { name: "sessions", mayUse: ["auth"] },
        { name: "aggregator", mayUse: [] },
        { name: "agent", mayUse: ["aggregator"] },
      ],
    },
    {
      name: "providers",
      crates: [
        { name: "provider", mayUse: ["sessions"] },
        { name: "impl-a", mayUse: ["provider", "sessions"] },
        { name: "impl-b", mayUse: ["provider"] },
        { name: "drive", mayUse: ["provider", "sessions"] },
      ],
    },
    { name: "entry", crates: [{ name: "server", mayUse: "*" }] },
  ],
  outside: { crates: ["relay", "devkit"] },
  forbidden: [
    { name: "the base names no implementation", why: "Each registers itself.", from: ["provider"], to: ["impl-a", "impl-b"] },
    { name: "siblings", why: "They do not see one another.", from: ["impl-a", "impl-b", "drive"], to: ["impl-a", "impl-b", "drive"] },
    { name: "the domain is under the providers", why: "Ports go down.", from: ["@domain"], to: ["@providers"] },
    { name: "the transport carries no app", why: "No app in the transport.", from: ["agent", "aggregator"], to: ["@domain"], except: ["agent -> aggregator"] },
  ],
  wrappers: [
    { crate: "portable-pty", why: "Terminals are pty's.", wrappers: ["pty"] },
    { crate: "tokio-tungstenite", why: "Sockets are the link's.", wrappers: ["ws-client", "agent", "relay"] },
  ],
};

/** A workspace that keeps every rule. */
const good = () => ({
  base: [],
  pty: ["base", "portable-pty"],
  "ws-client": ["base", "tokio-tungstenite"],
  auth: ["pty"],
  sessions: ["auth", "base"],
  aggregator: ["ws-client"],
  agent: ["aggregator", "tokio-tungstenite"],
  provider: ["sessions", "pty"],
  "impl-a": ["provider", "sessions"],
  "impl-b": ["provider"],
  drive: ["provider"],
  server: ["impl-a", "impl-b", "drive", "agent", "dev sessions"],
  relay: ["base", "tokio-tungstenite"],
  devkit: ["relay", "ws-client"],
});

/** `spec` changed by `edit`, checked, its members' `code` read as given. */
const check = (edit = () => {}, cfg = config, code) => {
  const spec = good();
  edit(spec);
  return checkCrates(workspace(spec), cfg, code);
};
const problems = (edit, cfg) => check(edit, cfg).problems;
const one = (found, pattern) => {
  assert.equal(found.filter((p) => pattern.test(p)).length, 1, `one problem matching ${pattern} in:\n${found.join("\n")}`);
};

describe("the crates as Cargo builds them", () => {
  test("a workspace that keeps the layers passes", () => {
    const { problems: found, stats } = check();
    assert.deepEqual(found, []);
    assert.deepEqual(stats.outside, ["relay", "devkit"]);
    assert.deepEqual(stats.waiting, []);
    assert.deepEqual(stats.forbidden.waiting, []);
  });

  test("every member is placed in a layer or listed outside them, not both", () => {
    one(problems((s) => (s.stray = [])), /crates\/stray\/Cargo\.toml: stray is a workspace member that layers\.json neither places in a layer nor lists outside the layers/);
    one(problems(undefined, { ...config, outside: { crates: ["relay", "devkit", "auth"] } }), /places auth in a layer and lists it outside the layers/);
  });

  test("a crate declared but not a member yet is left out, and so is a rule with no pair of members", () => {
    const { problems: found, stats } = check((s) => {
      delete s["impl-b"];
      delete s.drive;
      s.server = ["impl-a", "agent"];
    });
    assert.deepEqual(found, []);
    assert.deepEqual(stats.waiting, ["impl-b", "drive"]);
    assert.deepEqual(stats.forbidden.waiting, ['"siblings"']);
  });

  test("up a layer, or sideways past mayUse, fails and says which", () => {
    const found = problems((s) => {
      s.auth.push("sessions");
      s.base.push("pty");
    });
    one(found, /crates\/auth\/Cargo\.toml: auth's dependency sessions: auth's mayUse does not list sessions/);
    one(found, /base's dependency pty: pty is in a higher layer \(platform\) than base \(base\)/);
  });

  test("a lower layer that is not open is used only where mayUse lists it; an open one freely", () => {
    one(problems((s) => s.provider.push("auth")), /provider's dependency auth: provider's mayUse does not list auth/);
    assert.deepEqual(problems((s) => s.drive.push("pty", "base")), []);
  });

  test("dev- and build-dependencies are left out of the layer and wrapper rules", () => {
    assert.deepEqual(problems((s) => s.auth.push("dev sessions", "build sessions", "dev portable-pty", "build tokio-tungstenite")), []);
  });

  test("nothing depends on the entry, not even its tests", () => {
    const found = problems((s) => {
      s.auth.push("dev server");
      s.relay.push("server");
    });
    one(found, /auth's dev-dependency server: nothing depends on the entry/);
    one(found, /relay's dependency server: nothing depends on the entry/);
  });

  test("a crate of the layers uses none outside them; one outside uses the open layers and the others outside", () => {
    const found = problems((s) => {
      s.provider.push("relay");
      s.relay.push("auth");
    });
    one(found, /provider's dependency relay: relay is outside the layers, and no crate of the layers depends on one/);
    one(found, /relay's dependency auth: a crate outside the layers depends only on the open layers and on one another, and auth's layer \(domain\) is not open/);
  });

  test("a renamed or per-target dependency is the crate it names; a registry crate with a member's name is not the member", () => {
    one(problems((s) => s.sessions.push("p = provider")), /sessions's dependency p \(provider\): provider is in a higher layer/);
    one(problems((s) => s.auth.push("sessions for cfg(unix)")), /auth's dependency sessions for cfg\(unix\): auth's mayUse does not list sessions/);
    assert.deepEqual(problems((s) => s.sessions.push("registry provider")), []);
  });
});

describe("forbidden pairs", () => {
  test("a pair is reached through any chain of dependencies, and the failure gives the chain and the reason", () => {
    const chain = {
      layers: [{ name: "domain", crates: [{ name: "x", mayUse: ["y"] }, { name: "y", mayUse: ["z"] }, { name: "z", mayUse: [] }] }],
      forbidden: [{ name: "x never reaches z", why: "Because.", from: ["x"], to: ["z"] }],
    };
    const found = checkCrates(workspace({ x: ["y"], y: ["z"], z: [] }), chain).problems;
    assert.deepEqual(found, ['crates/x/Cargo.toml: x reaches z (x -> y -> z), which "x never reaches z" forbids: Because.']);
  });

  test("each rule of a made-up workspace: the base, the siblings, the domain and the transport", () => {
    const found = problems((s) => {
      s.provider.push("impl-a");
      s["impl-b"].push("drive");
      s.sessions.push("provider");
      s.aggregator.push("auth");
    });
    one(found, /provider reaches impl-a \(provider -> impl-a\), which "the base names no implementation" forbids: Each registers itself\./);
    one(found, /impl-b reaches drive \(impl-b -> drive\), which "siblings" forbids/);
    one(found, /sessions reaches provider \(sessions -> provider\), which "the domain is under the providers" forbids/);
    one(found, /aggregator reaches auth \(aggregator -> auth\), which "the transport carries no app" forbids/);
    one(found, /agent reaches auth \(agent -> aggregator -> auth\), which "the transport carries no app" forbids/);
  });

  test("a crate's own dev- and build-dependencies count, marked in the chain; its dependencies' do not", () => {
    const found = problems((s) => {
      s.sessions.push("dev provider");
      s["impl-a"].push("build impl-b");
      s.aggregator.push("dev sessions");
    });
    one(found, /sessions reaches provider \(sessions -> provider \(dev\)\), which "the domain is under the providers" forbids/);
    one(found, /impl-a reaches impl-b \(impl-a -> impl-b \(build\)\), which "siblings" forbids/);
    one(found, /aggregator reaches sessions \(aggregator -> sessions \(dev\)\), which "the transport carries no app" forbids: No app in the transport\. It reaches auth too\./);
    assert.equal(found.filter((p) => /^crates\/agent\//.test(p)).length, 0, "the aggregator's tests are not the agent's");
  });

  test("a crate that reaches several it may not is one line: the nearest with its chain, then the others", () => {
    const found = problems((s) => s.agent.push("sessions"));
    one(found, /agent reaches sessions \(agent -> sessions\), which "the transport carries no app" forbids: No app in the transport\. It reaches auth too\./);
    assert.equal(found.filter((p) => /"the transport carries no app"/.test(p)).length, 1);
  });

  test("except lets its pair through; an except that is not a pair, and a mayUse that lists a forbidden pair, fail", () => {
    assert.deepEqual(problems(), []);
    const bad = structuredClone(config);
    bad.forbidden[3].except.push("auth -> agent");
    bad.layers[2].crates[0].mayUse = ["aggregator"];
    const found = problems(undefined, bad);
    one(found, /"the transport carries no app" lets auth -> agent through, which is not one of its pairs/);
    assert.equal(found.filter((p) => /auth's mayUse lists aggregator/.test(p)).length, 0, "auth is not in the rule's from");
    bad.layers[3].crates[0].mayUse = ["sessions", "impl-a"];
    one(problems(undefined, bad), /provider's mayUse lists impl-a, which "the base names no implementation" forbids/);
  });

  test("a rule says its name and why, and names crates and layers layers.json has", () => {
    const bad = structuredClone(config);
    bad.forbidden.push({ from: ["nowhere"], to: ["@nolayer"] });
    const found = problems(undefined, bad);
    one(found, /forbidden\[4\] has no name/);
    one(found, /forbidden\[4\] does not say why/);
    one(found, /forbidden\[4\] names nowhere, which layers\.json does not list/);
    one(found, /forbidden\[4\] names @nolayer/);
  });
});

describe("wrappers", () => {
  test("only the listed members depend on the external crate, renamed or per target too; tests use it as they like", () => {
    const found = problems((s) => {
      s.sessions.push("portable-pty for cfg(unix)");
      s.auth.push("ws = tokio-tungstenite");
      s.drive.push("dev tokio-tungstenite");
    });
    one(found, /sessions's dependency portable-pty for cfg\(unix\): only pty may depend on portable-pty, so go through it\. Terminals are pty's\./);
    one(found, /auth's dependency ws \(tokio-tungstenite\): only ws-client, agent, relay may depend on tokio-tungstenite/);
    assert.equal(found.length, 2);
  });

  test("a listed member that no longer depends on it fails, so the list only shrinks; one that is no member yet is skipped", () => {
    one(
      problems((s) => (s.relay = ["base"])),
      /lets relay depend on tokio-tungstenite, which it no longer does: drop it/,
    );
    assert.deepEqual(
      problems((s) => {
        delete s.relay;
        s.devkit = ["ws-client"];
      }),
      [],
    );
  });
});

/** A member's code as {@link crateCode} reads it: `lines`, and a match of Source at each of `sources`. */
const read = (lines, sources = [], missing = []) => ({ lines, sources, missing });

describe("Source matches outside the provider crates", () => {
  const code = new Map([
    ["sessions", read(10, ["crates/sessions/src/lib.rs:4"])],
    ["drive", read(10, ["crates/drive/src/routes.rs:7", "crates/drive/src/routes.rs:9"])],
    ["impl-a", read(10, ["crates/impl-a/src/lib.rs:1", "crates/impl-a/src/lib.rs:2"])],
    ["gone", read(10, ["crates/gone/src/lib.rs:1"])],
  ]);
  const at = (count, exempt = [{ crate: "impl-a", why: "It answers for its Source." }]) => check(undefined, { ...config, sourceMatches: { exempt, count } }, code);

  test("are as many as recorded: more fails and names the files, fewer fails and says the number; exempt crates and non-members aside", () => {
    assert.deepEqual(at(3).problems, []);
    assert.equal(at(3).stats.sourceMatches, 3);
    one(at(2).problems, /3 matches of Source::Claude\|Codex\|Acp outside the provider crates, more than the 2 sourceMatches records: dispatch through the provider instead \(crates\/drive\/src\/routes\.rs 2, crates\/sessions\/src\/lib\.rs 1\)/);
    one(at(4).problems, /3 matches of Source::Claude\|Codex\|Acp outside the provider crates, fewer than the 4 sourceMatches records: lower it to 3/);
    one(at(3, []).problems, /5 matches .* more than the 3/);
  });

  test("an exemption says why, and names a crate layers.json lists", () => {
    const found = at(3, [{ crate: "impl-a" }, { crate: "nowhere", why: "No such crate." }]).problems;
    one(found, /sourceMatches exempts impl-a without saying why/);
    one(found, /sourceMatches exempts nowhere, which layers.json does not list/);
  });
});

describe("what layers.json no longer has", () => {
  test("no crate places modules and nothing lists exceptions between them: the server is the entry", () => {
    const stale = structuredClone(config);
    stale.allowed = [];
    stale.layers[2].crates[1].modules = ["sessions"];
    const found = problems(undefined, stale);
    one(found, /layers\.json has `allowed`, the exceptions between the server's modules, which nothing reads/);
    one(found, /layers\.json places modules in sessions \(`modules`\), which nothing reads/);
  });

  test("a member's `mod x;` with no file fails", () => {
    const code = new Map([["auth", read(1, [], [{ at: "crates/auth/src/lib.rs:3", name: "gone" }])]]);
    one(check(undefined, config, code).problems, /^crates\/auth\/src\/lib\.rs:3: mod gone; has no file$/);
  });
});

describe("what is read of a crate's code", () => {
  const named = (src) => [...builtCode(src).matchAll(/crate::(\w+)/g)].map((m) => m[1]);

  test("code counts; comments and strings do not", () => {
    const src = 'use crate::a::X; // crate::b\n/* crate::c /* nested */ crate::d */\nlet s = "crate::e"; let r = r#"crate::f "{" "#;\ncrate::g::h();\n';
    assert.deepEqual(named(src), ["a", "g"]);
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

  test("a member's library and binaries, through their modules built outside tests: its code lines and Source matches", () => {
    const root = mkdtempSync(join(tmpdir(), "check-layers-"));
    after(() => rmSync(root, { recursive: true, force: true }));
    const write = (path, text) => {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), text);
    };
    write("crates/the-pty/src/lib.rs", "pub mod ring;\nmod gone;\npub fn f() {}\n#[cfg(test)]\nmod tests {\n    fn t() { let _ = Source::Claude; }\n}\n#[cfg(test)]\nmod only_tests;\n");
    write("crates/the-pty/src/ring.rs", '// the ring, not Source::Acp\npub struct Ring;\npub fn pick() { let s = "Source::Acp"; match x { Source::Codex => 1 } }\n');
    write("crates/the-pty/src/only_tests.rs", "fn t() { let _ = Source::Claude; }\n");
    write("crates/the-pty/src/bin/tool.rs", "fn main() { let _ = Source::Acp; }\n");
    write("crates/the-pty/tests/it.rs", "fn t() { let _ = Source::Claude; }\n");
    const target = (kind, path) => ({ kind: [kind], src_path: join(root, path) });
    const meta = {
      workspace_root: root,
      workspace_members: ["the-pty#0.1.0", "elsewhere#0.1.0"],
      packages: [
        {
          id: "the-pty#0.1.0",
          name: "the-pty",
          manifest_path: join(root, "crates/the-pty/Cargo.toml"),
          targets: [target("lib", "crates/the-pty/src/lib.rs"), target("bin", "crates/the-pty/src/bin/tool.rs"), target("test", "crates/the-pty/tests/it.rs")],
        },
        { id: "not-a-member#0.1.0", name: "not-a-member", manifest_path: join(root, "x/Cargo.toml"), targets: [] },
      ],
    };
    const code = crateCode(meta);
    assert.deepEqual([...code.keys()], ["the-pty"]);
    assert.deepEqual(code.get("the-pty"), {
      // The library's: `pub mod ring;`, `mod gone;`, `pub fn f() {}`, and ring.rs's 2.
      lines: 5,
      // The match in code, the library's and the binary's; not the comment's,
      // the string's, the test module's or the test target's.
      sources: ["crates/the-pty/src/ring.rs:3", "crates/the-pty/src/bin/tool.rs:1"],
      missing: [{ at: "crates/the-pty/src/lib.rs:2", name: "gone" }],
    });
  });
});

describe("the numbers", () => {
  test("the largest cycle with the dev-dependencies, the middle crates' sizes and dependents, the Source count", () => {
    const code = new Map([["auth", read(120, ["crates/auth/src/lib.rs:1"])]]);
    const { stats } = check((s) => s.auth.push("dev sessions"), { ...config, sourceMatches: { exempt: [], count: 1 } }, code);
    assert.deepEqual(stats.cycle, ["auth", "sessions"]);
    assert.deepEqual(stats.middle.find((c) => c.crate === "auth"), { crate: "auth", code: 120, dependents: 1 });
    assert.deepEqual(stats.middle.find((c) => c.crate === "pty"), { crate: "pty", code: 0, dependents: 2 });
    assert.equal(stats.sourceMatches, 1);
  });
});

test("server-rs/layers.json: its outside crates, forbidden pairs, wrappers and exemptions name what it lists, no mayUse lists a forbidden pair, and nothing places a module", () => {
  const real = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "server-rs", "layers.json"), "utf8"));
  // With no members read, only the count can be off: the real one is the check's.
  const found = checkCrates({ packages: [], workspace_members: [] }, real).problems;
  assert.deepEqual(
    found.filter((p) => !/matches of Source::/.test(p)),
    [],
  );
});
