import assert from "node:assert/strict";
import test from "node:test";
import {
  conflictCurrent,
  definePlugin,
  isDocName,
  isPluginId,
  isStorageError,
  localize,
  parseVersion,
  satisfies,
  SDK_VERSION,
  validRange,
} from "../src/index";
import { createTestHost } from "../src/testing";

test("plugin ids are lower-case, start with a letter and fit a directory name", () => {
  for (const id of ["tasks", "my-plugin", "a1"]) assert.ok(isPluginId(id), id);
  for (const id of ["Tasks", "../x", "1abc", "a", "a_b", "x".repeat(33), ""]) assert.ok(!isPluginId(id), id);
});

test("document names are slash-separated safe segments", () => {
  for (const name of ["state", "events/2026-10", "reports/2026-10-12", "a.b_c-d"]) assert.ok(isDocName(name), name);
  for (const name of ["", "/state", "state/", "a//b", "../x", "a/./b", "a b", "名", "x".repeat(201)]) assert.ok(!isDocName(name), name);
});

test("engine ranges read exactly as the host's Rust side reads them", async () => {
  const { readFileSync } = await import("node:fs");
  const vectors = JSON.parse(readFileSync(new URL("./engine-vectors.json", import.meta.url), "utf8")) as {
    matches: [string, string, boolean][];
    invalidRanges: string[];
    validVersions: string[];
    invalidVersions: string[];
  };
  for (const [range, version, expected] of vectors.matches) {
    assert.ok(validRange(range), `${range} parses`);
    assert.equal(satisfies(version, range), expected, `${range} against ${version}`);
  }
  for (const range of vectors.invalidRanges) assert.ok(!validRange(range), `${JSON.stringify(range)} is refused`);
  for (const version of vectors.validVersions) assert.ok(parseVersion(version), version);
  for (const version of vectors.invalidVersions) assert.equal(parseVersion(version), null, JSON.stringify(version));
  assert.ok(satisfies(SDK_VERSION, "^0.1.0"));
  assert.ok(satisfies("0.1.5", "0.1.0"), "a bare version is a caret requirement, as in Cargo");
  assert.ok(!satisfies("0.2.0", "^0.1.0"), "every 0.x minor may break");
});

test("localized text falls back to English, and zh-TW to zh-CN", () => {
  const name = { en: "Tasks", "zh-CN": "待办" };
  assert.equal(localize(name, "zh-CN"), "待办");
  assert.equal(localize(name, "zh-TW"), "待办");
  assert.equal(localize(name, "ja"), "Tasks");
  assert.equal(localize("plain", "ja"), "plain");
});

test("definePlugin hands the definition back unchanged", () => {
  const def = { manifest: { id: "x1", version: "1.0.0", name: "X", engines: { droppen: "^0.1.0" } }, activate: () => ({}) };
  assert.equal(definePlugin(def), def);
});

test("the test host's storage enforces compare-and-set like a real host", async () => {
  const host = createTestHost({ pluginId: "tasks", docs: { state: { n: 1 } } });
  const first = await host.storage.get<{ n: number }>("state");
  assert.equal(first?.revision, 1);
  const second = await host.storage.put("state", { n: 2 }, 1);
  assert.equal(second.revision, 2);
  await assert.rejects(host.storage.put("state", { n: 3 }, 1), (error: unknown) => {
    assert.ok(isStorageError(error, "conflict"));
    assert.deepEqual(conflictCurrent<{ n: number }>(error)?.value, { n: 2 });
    return true;
  });
  await assert.rejects(host.storage.put("fresh", {}, 4), (error: unknown) => isStorageError(error, "conflict") && conflictCurrent(error) === null);
  const created = await host.storage.put("fresh", { ok: true }, null);
  assert.equal(created.revision, 1);
  await assert.rejects(host.storage.put("fresh", {}, null), (error: unknown) => isStorageError(error, "conflict"));
});

test("a batch is all or nothing", async () => {
  const host = createTestHost({ docs: { state: 1 } });
  await assert.rejects(
    host.storage.batch([
      { name: "events/2026-10", value: [1], expectedRevision: null },
      { name: "state", value: 2, expectedRevision: 7 },
    ]),
    (error: unknown) => isStorageError(error, "conflict"),
  );
  assert.equal(await host.storage.get("events/2026-10"), null, "the first write did not land");
  const docs = await host.storage.batch([
    { name: "events/2026-10", value: [1], expectedRevision: null },
    { name: "state", value: 2, expectedRevision: 1 },
  ]);
  assert.deepEqual(
    docs.map((d) => [d.name, d.revision]),
    [
      ["events/2026-10", 1],
      ["state", 2],
    ],
  );
  assert.deepEqual(
    (await host.storage.list("events/")).map((d) => d.name),
    ["events/2026-10"],
  );
});

test("watchers hear writes from this tab and from elsewhere", async () => {
  const host = createTestHost({ docs: { state: 0 } });
  const heard: (number | null)[] = [];
  const stop = host.storage.watch("state", (revision) => heard.push(revision));
  await host.storage.put("state", 1, 1);
  host.writeElsewhere("state", 2);
  stop();
  host.writeElsewhere("state", 3);
  assert.deepEqual(heard, [2, 3]);
});

test("oversized documents and injected failures surface as storage errors", async () => {
  const host = createTestHost({ docLimit: 10 });
  await assert.rejects(host.storage.put("big", "x".repeat(20), null), (error: unknown) => isStorageError(error, "too_large"));
  host.failNext({ code: "offline", message: "machine offline" });
  await assert.rejects(host.storage.get("x"), (error: unknown) => isStorageError(error, "offline"));
  assert.equal(await host.storage.get("x"), null, "only once");
});

test("host side effects are recorded rather than performed", async () => {
  const host = createTestHost({ confirm: (message) => message.includes("yes"), strings: { hello: "你好 {{name}}" } });
  assert.equal(await host.ui.confirm("say yes"), true);
  assert.equal(await host.ui.confirm("say no"), false);
  assert.deepEqual(host.confirms, ["say yes", "say no"]);
  await host.files.save(new Blob(["x"]), "a.txt");
  await host.print({ title: "t", html: "<p>x</p>" });
  await host.clipboard.writeText("copied");
  host.navigation.navigate("weekly");
  assert.equal(host.saved[0]?.name, "a.txt");
  assert.equal(host.printed[0]?.title, "t");
  assert.deepEqual(host.copied, ["copied"]);
  assert.deepEqual(host.navigations, ["weekly"]);
  assert.equal(host.i18n.t("hello", { name: "陈" }), "你好 陈");
  assert.equal(host.i18n.t("missing"), "missing");
});

test("the SDK version is the same in the package, the constant and the Rust crate", async () => {
  const { readFileSync } = await import("node:fs");
  const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as { version: string };
  assert.equal(pkg.version, SDK_VERSION);
  const crate = readFileSync(new URL("../../../server-rs/crates/droppen-plugin-sdk/src/lib.rs", import.meta.url), "utf8");
  assert.match(crate, new RegExp(`pub const SDK_VERSION: &str = "${SDK_VERSION.replaceAll(".", "\\.")}";`));
});

/**
 * The React major each SDK version lends a plugin. A plugin's bare `react` is
 * the host's (the web build's import map), so React is part of what
 * `SDK_VERSION` versions: a new React major is a breaking SDK change, and
 * takes the next breaking version — a minor while on 0.x — and a row here
 * (`docs/plugins/runtime.md` §3).
 */
const REACT_OF_SDK: Record<string, number> = { "0.1": 19 };

test("the host's React major is the one its SDK version promises plugins", async () => {
  const { readFileSync } = await import("node:fs");
  const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8")) as Record<string, Record<string, string>>;
  const major = (range: string | undefined) => Number(/^\^?(\d+)\./.exec(range ?? "")?.[1]);
  const [maj, min] = SDK_VERSION.split(".").map(Number);
  // Breaking on 0.x is a minor, from 1.0 a major (Cargo's caret rules).
  const line = maj === 0 ? `0.${min}` : `${maj}`;
  const promised = REACT_OF_SDK[line];
  assert.ok(promised, `SDK ${SDK_VERSION} names no React major: add "${line}" to REACT_OF_SDK`);
  const web = major(read("../../../web/package.json").dependencies?.react);
  assert.equal(
    web,
    promised,
    `the web build lends plugins React ${web}, but SDK ${line} promises React ${promised}: a React major is a breaking SDK change — bump SDK_VERSION (here, the package and the Rust crate) and add its row`,
  );
  const pkg = read("../package.json");
  assert.equal(major(pkg.peerDependencies?.react), promised, "the SDK's react peer range is the React it promises");
  assert.equal(major(pkg.devDependencies?.react), promised, "and the one its tests run on");
});
