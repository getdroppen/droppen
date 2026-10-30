// Tests for scripts/check-crate-layers.mjs: `node --test scripts/check-crate-layers.test.mjs`.
//
// The crate rules over made-up workspaces, and server-rs/layers.json's own
// rules as the check reads them. The real thing is `pnpm check:crate-layers`.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, test } from "node:test";
import { fileURLToPath } from "node:url";
import { checkCrates } from "./check-crate-layers.mjs";

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

/** `spec` changed by `edit`, checked. */
const check = (edit = () => {}, cfg = config, options) => {
  const spec = good();
  edit(spec);
  return checkCrates(workspace(spec), cfg, options);
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

describe("the numbers", () => {
  test("the largest cycle with the dev-dependencies, the middle crates' sizes and dependents", () => {
    const { stats } = check((s) => s.auth.push("dev sessions"), config, { sizes: new Map([["auth", 120]]), sourceMatches: 7 });
    assert.deepEqual(stats.cycle, ["auth", "sessions"]);
    assert.deepEqual(stats.middle.find((c) => c.crate === "auth"), { crate: "auth", code: 120, dependents: 1 });
    assert.deepEqual(stats.middle.find((c) => c.crate === "pty"), { crate: "pty", code: 0, dependents: 2 });
    assert.equal(stats.sourceMatches, 7);
  });
});

test("server-rs/layers.json: its outside crates, forbidden pairs and wrappers name what it lists, and no mayUse lists a forbidden pair", () => {
  const real = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "server-rs", "layers.json"), "utf8"));
  assert.deepEqual(checkCrates({ packages: [], workspace_members: [] }, real).problems, []);
});
