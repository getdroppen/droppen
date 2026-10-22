// Tests for scripts/publish-release.sh: `node --test scripts/publish-release.test.mjs`.
//
// The script runs against stand-ins: a `gh` that keeps a fake repository's
// releases in a JSON file and logs every call, a `curl` that serves its latest
// release's latest.json, an `xcrun` whose stapler accepts a file that says
// "stapled", and a manifest checker that refuses an archive that says
// "tampered". They come first on a PATH of their own, in an environment of its
// own (no real gh login, a repository that does not exist), and every test
// checks that `gh` and `curl` are the stand-ins before it runs anything:
// nothing here can reach GitHub.

import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, test } from "node:test";
import { fileURLToPath } from "node:url";

const SCRIPT = process.env.PUBLISH_RELEASE_SCRIPT ?? fileURLToPath(new URL("./publish-release.sh", import.meta.url));
const REPO = "droppen-test/no-such-releases";
const sha256 = (data) => createHash("sha256").update(data).digest("hex");

// The stand-in gh: releases in $FAKE_GH_STATE (newest first), asset bytes
// under $FAKE_GH_FILES/<tag>/<name>, every call appended to $FAKE_GH_LOG.
// FAKE_GH_DROP names an asset that an upload silently loses.
const FAKE_GH = String.raw`
const fs = require("fs"), path = require("path");
const args = process.argv.slice(2);
const statePath = process.env.FAKE_GH_STATE, files = process.env.FAKE_GH_FILES;
fs.appendFileSync(process.env.FAKE_GH_LOG, JSON.stringify(args) + "\n");
const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
const save = () => fs.writeFileSync(statePath, JSON.stringify(state, null, 2));
const fail = (msg) => { process.stderr.write(msg + "\n"); process.exit(1); };
const flag = (name) => { const i = args.indexOf(name); return i < 0 ? undefined : args[i + 1]; };
const repo = flag("--repo");
if (args[0] === "api") {
  const endpoint = args.find((a) => a.startsWith("repos/"));
  if (endpoint === "repos/" + process.env.REPO + "/releases") {
    if (!args.includes("--paginate") || !args.includes("--slurp")) fail("list without --paginate --slurp");
    const strip = (r) => ({ ...r, assets: r.assets.map(({ name, size, state }) => ({ name, size, state })) });
    process.stdout.write(JSON.stringify([state.releases.map(strip)]));
  } else if (endpoint === "repos/" + process.env.REPO + "/releases/latest") {
    if (process.env.FAKE_GH_LATEST_FAILS) fail(process.env.FAKE_GH_LATEST_FAILS);
    if (!state.latest) fail("gh: Not Found (HTTP 404)");
    process.stdout.write(args.includes("--jq") ? state.latest + "\n" : JSON.stringify({ tag_name: state.latest }));
  } else fail("unexpected api call " + endpoint);
  process.exit(0);
}
if (args[0] !== "release") fail("unexpected gh " + args.join(" "));
if (repo !== process.env.REPO) fail("wrong repo " + repo);
const tag = args[2];
const find = (t) => state.releases.find((r) => r.tag_name === t);
const positional = () => {
  const out = [];
  for (let i = 3; i < args.length; i++) {
    if (args[i].startsWith("--")) { if (!args[i].includes("=") && !["--draft", "--prerelease", "--latest", "--verify-tag"].includes(args[i])) i++; continue; }
    out.push(args[i]);
  }
  return out;
};
const attach = (release, paths) => {
  fs.mkdirSync(path.join(files, release.tag_name), { recursive: true });
  for (const p of paths) {
    const name = path.basename(p);
    if (name === process.env.FAKE_GH_DROP) continue;
    if (release.assets.some((a) => a.name === name)) fail("asset " + name + " already exists");
    fs.copyFileSync(p, path.join(files, release.tag_name, name));
    release.assets.push({ name, size: fs.statSync(p).size, state: "uploaded" });
  }
};
switch (args[1]) {
  case "download": {
    const release = find(tag);
    if (!release || release.draft) fail("release not found");
    const name = flag("--pattern");
    if (!release.assets.some((a) => a.name === name)) fail("no assets match the file pattern");
    fs.mkdirSync(flag("--dir"), { recursive: true });
    fs.copyFileSync(path.join(files, tag, name), path.join(flag("--dir"), name));
    break;
  }
  case "create": {
    if (find(tag)) fail("a release " + tag + " exists");
    const release = { tag_name: tag, draft: args.includes("--draft"), prerelease: args.includes("--prerelease"), assets: [] };
    if (!release.draft) state.latest = tag;
    state.releases.unshift(release);
    attach(release, positional());
    break;
  }
  case "upload": {
    const release = find(tag);
    if (!release) fail("release not found");
    attach(release, positional());
    break;
  }
  case "edit": {
    const release = find(tag);
    if (!release) fail("release not found");
    if (args.includes("--draft=false")) release.draft = false;
    if (args.includes("--latest")) state.latest = tag;
    break;
  }
  default: fail("unexpected gh release " + args[1]);
}
save();
`;

const FAKE_CURL = String.raw`
const fs = require("fs"), path = require("path");
const args = process.argv.slice(2);
const out = args[args.indexOf("-o") + 1], url = args[args.length - 1];
const state = JSON.parse(fs.readFileSync(process.env.FAKE_GH_STATE, "utf8"));
if (url !== "https://github.com/" + process.env.REPO + "/releases/latest/download/latest.json" || !state.latest) process.exit(22);
const file = path.join(process.env.FAKE_GH_FILES, state.latest, "latest.json");
const release = state.releases.find((r) => r.tag_name === state.latest);
if (!release.assets.some((a) => a.name === "latest.json")) process.exit(22);
if (process.env.FAKE_CURL_STALE) fs.writeFileSync(out, "{}");
else fs.copyFileSync(file, out);
`;

const FAKE_XCRUN = String.raw`
const fs = require("fs");
const [tool, verb, file] = process.argv.slice(2);
process.exit(tool === "stapler" && verb === "validate" && fs.readFileSync(file, "utf8").includes("stapled") ? 0 : 65);
`;

const FAKE_UNAME = String.raw`
process.stdout.write((process.env.FAKE_UNAME || "Darwin") + "\n");
`;

// The manifest checker's stand-in: the real one is tested in desktop/core.
const FAKE_MANIFEST_TOOL = String.raw`
const fs = require("fs");
const [command, file] = process.argv.slice(2);
const flag = (name) => process.argv[process.argv.indexOf(name) + 1];
fs.appendFileSync(process.env.FAKE_TOOL_LOG, JSON.stringify(process.argv.slice(2)) + "\n");
// key-id: an id that follows the key file's bytes.
if (command === "key-id") {
  const bytes = fs.readFileSync(flag("--pubkey"));
  console.log(require("crypto").createHash("sha256").update(bytes).digest("hex").slice(0, 16).toUpperCase());
  process.exit(0);
}
if (command !== "check" || !fs.existsSync(flag("--pubkey"))) process.exit(2);
const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
if (fs.readFileSync(flag("--archive"), "utf8").includes("tampered")) {
  process.stderr.write("refused: the signature does not verify the archive with this key\n");
  process.exit(1);
}
console.log(file + ": Droppen " + manifest.version + " is fit to publish");
`;

function manifest(version) {
  const platform = { signature: `SIG-${version}`, url: `https://github.com/getdroppen/droppen/releases/download/v${version}/Droppen.app.tar.gz` };
  return `${JSON.stringify(
    {
      version,
      notes: `https://github.com/getdroppen/droppen/releases/tag/v${version}`,
      pub_date: "2026-10-20T08:00:00Z",
      platforms: { "darwin-aarch64": platform, "darwin-x86_64": platform },
    },
    null,
    2,
  )}\n`;
}

let root, env, fakes, stateFile, filesDir, logFile;

/** A release of the fake repository, its assets written under filesDir. */
function release(tag, assets, { draft = false, prerelease = false } = {}) {
  mkdirSync(join(filesDir, tag), { recursive: true });
  return {
    tag_name: tag,
    draft,
    prerelease,
    assets: Object.entries(assets).map(([name, content]) => {
      writeFileSync(join(filesDir, tag, name), content);
      return { name, size: Buffer.byteLength(content), state: "uploaded" };
    }),
  };
}

function setState(releases, latest) {
  writeFileSync(stateFile, JSON.stringify({ releases, latest }, null, 2));
}
const state = () => JSON.parse(readFileSync(stateFile, "utf8"));
const calls = () =>
  existsSync(logFile)
    ? readFileSync(logFile, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line))
    : [];
const writes = () => calls().filter((c) => c[0] === "release" && ["create", "upload", "edit"].includes(c[1]));

/** release-public.sh's --out for `tag`. */
function serverDir(tag) {
  const dir = join(root, `server-${tag}`);
  mkdirSync(dir);
  const sums = [];
  for (const name of [`droppen-${tag}-macos-universal.tar.gz`, `remote-hand-${tag}-macos-universal.tar.gz`]) {
    const content = `tarball ${name}`;
    writeFileSync(join(dir, name), content);
    sums.push(`${sha256(content)}  ./${name}`);
  }
  writeFileSync(join(dir, "SHA256SUMS"), `${sums.join("\n")}\n`);
  for (const name of ["NOTES.md", "LICENSE", "THIRD_PARTY_NOTICES", "web-manifest.json"]) writeFileSync(join(dir, name), `${name}\n`);
  return dir;
}

/** desktop/build.sh's --out after SIGNING.md, for x.y.z `version`. */
function desktopDir(version, { dmg = "stapled dmg", announced = version } = {}) {
  const dir = join(root, `desktop-${version}`);
  mkdirSync(join(dir, "updater"), { recursive: true });
  writeFileSync(join(dir, `Droppen-${version}.dmg`), dmg);
  writeFileSync(join(dir, "updater", "Droppen.app.tar.gz"), `archive ${version}`);
  writeFileSync(join(dir, "updater", "Droppen.app.tar.gz.sig"), `SIG-${announced}\n`);
  writeFileSync(join(dir, "updater", "latest.json"), manifest(announced));
  return dir;
}

function run(args) {
  return new Promise((resolve) => {
    execFile("bash", [SCRIPT, ...args], { env, cwd: root }, (error, stdout, stderr) =>
      resolve({ code: error ? error.code : 0, stdout, stderr }),
    );
  });
}

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "publish-release-test-"));
  fakes = join(root, "fakes");
  filesDir = join(root, "files");
  stateFile = join(root, "state.json");
  logFile = join(root, "gh.log");
  mkdirSync(fakes);
  mkdirSync(filesDir);
  mkdirSync(join(root, "home"));
  mkdirSync(join(root, "tmp"));
  // node itself, under its own name, so the script's `node` is this one.
  symlinkSync(process.execPath, join(fakes, "node"));
  for (const [name, source] of [
    ["gh", FAKE_GH],
    ["curl", FAKE_CURL],
    ["xcrun", FAKE_XCRUN],
    ["uname", FAKE_UNAME],
    ["droppen-update-manifest", FAKE_MANIFEST_TOOL],
  ]) {
    writeFileSync(join(fakes, name), `#!${process.execPath}\n${source}`);
    chmodSync(join(fakes, name), 0o755);
  }
  env = {
    PATH: `${fakes}:/usr/bin:/bin`,
    HOME: join(root, "home"),
    TMPDIR: join(root, "tmp"),
    GH_CONFIG_DIR: join(root, "home", "gh"),
    GH_TOKEN: "not-a-token",
    DROPPEN_RELEASE_REPO: REPO,
    REPO,
    DROPPEN_UPDATE_MANIFEST: join(fakes, "droppen-update-manifest"),
    DROPPEN_PUBLISH_RETRY_SECONDS: "0",
    FAKE_GH_STATE: stateFile,
    FAKE_GH_FILES: filesDir,
    FAKE_GH_LOG: logFile,
    FAKE_TOOL_LOG: join(root, "tool.log"),
  };
  // Never a real gh or curl.
  for (const tool of ["gh", "curl", "node", "uname", "xcrun"]) {
    const found = execFileSync("/bin/sh", ["-c", `command -v ${tool}`], { env }).toString().trim();
    assert.equal(found, join(fakes, tool), `${tool} must be the stand-in`);
  }
  // Today: v0.2.125 is the latest release and has the desktop app; v0.2.124 had none.
  setState(
    [
      release("v0.2.125", {
        "Droppen-0.2.125.dmg": "stapled dmg 0.2.125",
        "Droppen.app.tar.gz": "archive 0.2.125",
        "Droppen.app.tar.gz.sig": "SIG-0.2.125\n",
        "latest.json": manifest("0.2.125"),
      }),
      release("v0.2.124", { "Droppen-0.2.124.dmg": "dmg 0.2.124" }),
    ],
    "v0.2.125",
  );
});

afterEach(() => rmSync(root, { recursive: true, force: true }));

describe("publishing a release", () => {
  test("with the desktop app: a draft first, latest.json uploaded last and alone, then published as latest", async () => {
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126")]);
    assert.equal(r.code, 0, r.stderr);
    const w = writes();
    assert.deepEqual(
      w.map((c) => c.slice(0, 2)),
      [["release", "create"], ["release", "upload"], ["release", "upload"], ["release", "edit"]],
    );
    const [create, desktop, last, edit] = w;
    assert.ok(create.includes("--draft"), "created as a draft");
    assert.ok(!create.some((a) => a.endsWith("latest.json")), "no latest.json in the draft's first files");
    assert.ok(create.some((a) => a.endsWith("/SHA256SUMS")));
    assert.deepEqual(
      desktop.filter((a) => a.startsWith("/")).map((a) => a.split("/").pop()),
      ["Droppen-0.2.126.dmg", "Droppen.app.tar.gz", "Droppen.app.tar.gz.sig"],
    );
    assert.deepEqual(last.filter((a) => a.startsWith("/")).map((a) => a.split("/").pop()), ["latest.json"]);
    assert.ok(edit.includes("--draft=false") && edit.includes("--latest"));

    const s = state();
    assert.equal(s.latest, "v0.2.126");
    const published = s.releases.find((x) => x.tag_name === "v0.2.126");
    assert.equal(published.draft, false);
    assert.equal(published.assets.at(-1).name, "latest.json", "the last asset");
    assert.equal(readFileSync(join(filesDir, "v0.2.126", "latest.json"), "utf8"), manifest("0.2.126"));
    const sums = readFileSync(join(filesDir, "v0.2.126", "SHA256SUMS"), "utf8");
    for (const name of ["droppen-v0.2.126-macos-universal.tar.gz", "Droppen-0.2.126.dmg", "Droppen.app.tar.gz.sig", "latest.json"]) {
      assert.match(sums, new RegExp(`  \\./${name.replaceAll(".", "\\.")}\\n`));
    }
    assert.match(r.stdout, /v0\.2\.126 is the latest release and serves this latest\.json \(Droppen 0\.2\.126\)/);
  });

  test("without it: the latest release's latest.json is carried forward byte for byte, verified against its archive", async () => {
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126")]);
    assert.equal(r.code, 0, r.stderr);
    const downloads = calls().filter((c) => c[1] === "download");
    assert.deepEqual(
      downloads.map((c) => [c[2], c[c.indexOf("--pattern") + 1]]),
      [["v0.2.125", "latest.json"], ["v0.2.125", "Droppen.app.tar.gz"], ["v0.2.125", "Droppen-0.2.125.dmg"]],
    );
    const w = writes();
    assert.deepEqual(
      w.map((c) => c[1]),
      ["create", "upload", "upload", "edit"],
    );
    // The download page's DMG first, then latest.json, last and alone.
    assert.deepEqual(w[1].filter((a) => a.startsWith("/")).map((a) => a.split("/").pop()), ["Droppen-0.2.125.dmg"]);
    assert.deepEqual(w[2].filter((a) => a.startsWith("/")).map((a) => a.split("/").pop()), ["latest.json"]);
    for (const name of ["latest.json", "Droppen-0.2.125.dmg"]) {
      assert.equal(readFileSync(join(filesDir, "v0.2.126", name), "utf8"), readFileSync(join(filesDir, "v0.2.125", name), "utf8"));
    }
    assert.equal(state().releases.find((x) => x.tag_name === "v0.2.126").assets.at(-1).name, "latest.json");
    assert.equal(state().latest, "v0.2.126");
    assert.match(r.stdout, /v0\.2\.125's latest\.json \(Droppen 0\.2\.125\) goes on v0\.2\.126 unchanged/);
  });

  test("an older manifest is carried from the release of the version it announces", async () => {
    // v0.2.126 was server-only and carried v0.2.125's manifest.
    const s = state();
    s.releases.unshift(release("v0.2.126", { "latest.json": manifest("0.2.125") }));
    setState(s.releases, "v0.2.126");
    const r = await run(["v0.2.127", "--server", serverDir("v0.2.127")]);
    assert.equal(r.code, 0, r.stderr);
    assert.deepEqual(
      calls()
        .filter((c) => c[1] === "download")
        .map((c) => c[2]),
      ["v0.2.126", "v0.2.125"],
    );
    assert.match(r.stdout, /WARNING: v0\.2\.126 has no Droppen-<version>\.dmg/);
  });

  test("a dry run reads and checks everything, writes nothing, and prints the writes in order", async () => {
    const before = readFileSync(stateFile, "utf8");
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--dry-run"]);
    assert.equal(r.code, 0, r.stderr);
    assert.deepEqual(writes(), []);
    assert.equal(readFileSync(stateFile, "utf8"), before);
    const planned = r.stdout.split("\n").filter((line) => line.startsWith("would run:"));
    assert.equal(planned.length, 4, r.stdout);
    assert.match(planned[0], /^would run: gh release create v0\.2\.126 .*--draft/);
    assert.match(planned[1], /^would run: gh release upload v0\.2\.126 .*\/Droppen-0\.2\.125\.dmg$/);
    assert.match(planned[2], /^would run: gh release upload v0\.2\.126 .*\/latest\.json$/);
    assert.match(planned[3], /^would run: gh release edit v0\.2\.126 .*--draft=false --latest$/);
    assert.match(r.stdout, /nothing was written/);
  });

  test("a prerelease is published as one: no latest.json, never the latest", async () => {
    const r = await run(["v0.2.126-rc.1", "--server", serverDir("v0.2.126-rc.1")]);
    assert.equal(r.code, 0, r.stderr);
    const w = writes();
    assert.deepEqual(
      w.map((c) => c[1]),
      ["create", "edit"],
    );
    assert.ok(w[0].includes("--draft") && w[0].includes("--prerelease"));
    assert.ok(w[1].includes("--latest=false"));
    assert.equal(state().latest, "v0.2.125");
    assert.ok(!existsSync(join(filesDir, "v0.2.126-rc.1", "latest.json")));
  });

  test("--no-manifest, before any release has latest.json: none, and the DMG carried forward", async () => {
    setState([release("v0.2.124", { "Droppen-0.2.124.dmg": "dmg 0.2.124, before notarisation" })], "v0.2.124");
    const r = await run(["v0.2.125", "--server", serverDir("v0.2.125"), "--no-manifest"]);
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /no latest\.json: no published release has had one yet/);
    const w = writes();
    assert.deepEqual(
      w.map((c) => c[1]),
      ["create", "upload", "edit"],
    );
    assert.deepEqual(w[1].filter((a) => a.startsWith("/")).map((a) => a.split("/").pop()), ["Droppen-0.2.124.dmg"]);
    assert.equal(state().latest, "v0.2.125");
    assert.ok(!state().releases[0].assets.some((a) => a.name === "latest.json"));
  });
});

describe("what it refuses, before writing anything", () => {
  /** Runs, and checks that it failed with `message` and wrote nothing. */
  async function refused(args, message) {
    const r = await run(args);
    assert.notEqual(r.code, 0, r.stdout);
    assert.match(r.stderr, message);
    assert.deepEqual(writes(), []);
    return r;
  }

  test("notes that announce this version's app, without the app", async () => {
    const dir = serverDir("v0.2.126");
    writeFileSync(join(dir, "NOTES.md"), "## Droppen App for macOS\n\n`Droppen-0.2.126.dmg` — the desktop app. From 0.2.126 on, the app updates itself.\n");
    await refused(["v0.2.126", "--server", dir], /NOTES\.md announces Droppen-0\.2\.126\.dmg, but there is no --desktop/);
    // With the app, the same notes go out.
    const r = await run(["v0.2.126", "--server", dir, "--desktop", desktopDir("0.2.126")]);
    assert.equal(r.code, 0, r.stderr);
    assert.equal(state().latest, "v0.2.126");
  });

  test("a server-only release whose notes name only an earlier app is not refused for it", async () => {
    const dir = serverDir("v0.2.126");
    writeFileSync(join(dir, "NOTES.md"), "A server fix. The app stays `Droppen-0.2.125.dmg`.\n");
    const r = await run(["v0.2.126", "--server", dir]);
    assert.equal(r.code, 0, r.stderr);
  });

  test("a latest release without latest.json, naming the newest release that has one", async () => {
    const s = state();
    s.releases.unshift(release("v0.2.126", { "droppen-v0.2.126-macos-universal.tar.gz": "x" }));
    setState(s.releases, "v0.2.126");
    await refused(["v0.2.127", "--server", serverDir("v0.2.127")], /v0\.2\.126 carries no latest\.json.*--manifest-from v0\.2\.125/s);
    // Carried from there when asked.
    const r = await run(["v0.2.127", "--server", join(root, "server-v0.2.127"), "--manifest-from", "v0.2.125"]);
    assert.equal(r.code, 0, r.stderr);
    assert.equal(state().latest, "v0.2.127");
  });

  test("--no-manifest once a release has latest.json", async () => {
    await refused(["v0.2.126", "--server", serverDir("v0.2.126"), "--no-manifest"], /--no-manifest, but v0\.2\.125 has a latest\.json/);
  });

  test("a version that is not newer than the latest release", async () => {
    await refused(["v0.2.123", "--server", serverDir("v0.2.123")], /v0\.2\.123 is not newer than the latest release, v0\.2\.125/);
    await refused(
      ["v0.2.123", "--server", join(root, "server-v0.2.123"), "--desktop", desktopDir("0.2.123")],
      /not newer than the latest release/,
    );
  });

  test("--manifest-from while the latest release has its own", async () => {
    await refused(["v0.2.126", "--server", serverDir("v0.2.126"), "--manifest-from", "v0.2.124"], /the latest release, v0\.2\.125, has a latest\.json/);
  });

  test("a carried DMG of another version than the manifest, or without its ticket (on a Mac)", async () => {
    const s = state();
    const v125 = s.releases.find((x) => x.tag_name === "v0.2.125");
    v125.assets = v125.assets.filter((a) => a.name !== "Droppen-0.2.125.dmg");
    v125.assets.push(release("v0.2.125", { "Droppen-0.2.124.dmg": "stapled dmg 0.2.124" }).assets[0]);
    setState(s.releases, "v0.2.125");
    await refused(["v0.2.126", "--server", serverDir("v0.2.126")], /has Droppen-0\.2\.124\.dmg, but its latest\.json announces 0\.2\.125/);
    {
      const t = state();
      const again = t.releases.find((x) => x.tag_name === "v0.2.125");
      again.assets = again.assets.filter((a) => a.name !== "Droppen-0.2.124.dmg");
      again.assets.push(release("v0.2.125", { "Droppen-0.2.125.dmg": "dmg, never notarised" }).assets[0]);
      setState(t.releases, "v0.2.125");
      await refused(["v0.2.126", "--server", join(root, "server-v0.2.126")], /Droppen-0\.2\.125\.dmg has no stapled notarisation ticket/);
    }
  });

  test("no release has had latest.json yet: --no-manifest is the way", async () => {
    setState([release("v0.2.124", { "Droppen-0.2.124.dmg": "dmg" })], "v0.2.124");
    await refused(["v0.2.125", "--server", serverDir("v0.2.125")], /No published release has had one yet: pass --no-manifest/);
  });

  test("a prerelease or a draft as the manifest source", async () => {
    // The latest release, v0.2.126, has none, so --manifest-from is asked for.
    const s = state();
    s.releases.unshift(
      release("v0.2.127-rc.1", { "latest.json": manifest("0.2.125") }, { prerelease: true }),
      release("v0.2.127-draft", { "latest.json": manifest("0.2.125") }, { draft: true }),
      release("v0.2.126", { "droppen-v0.2.126-macos-universal.tar.gz": "x" }),
    );
    setState(s.releases, "v0.2.126");
    const server = serverDir("v0.2.127");
    await refused(["v0.2.127", "--server", server, "--manifest-from", "v0.2.127-rc.1"], /is a prerelease/);
    await refused(["v0.2.127", "--server", server, "--manifest-from", "v0.2.127-draft"], /is a draft/);
  });

  test("a carried manifest whose archive is on a prerelease", async () => {
    const s = state();
    s.releases[0].prerelease = true; // v0.2.125 has the archive
    s.releases.unshift(release("v0.2.126", { "latest.json": manifest("0.2.125") }));
    setState(s.releases, "v0.2.126");
    await refused(["v0.2.127", "--server", serverDir("v0.2.127")], /archive of v0\.2\.125, which is a draft or a prerelease/);
  });

  test("a carried manifest that is not older than the release", async () => {
    await refused(["v0.2.125", "--server", serverDir("v0.2.125"), "--manifest-from", "v0.2.125"], /already has a release v0\.2\.125/);
    const s = state();
    s.releases.unshift(release("v0.2.126", { "latest.json": manifest("0.2.127") }));
    setState(s.releases, "v0.2.126");
    await refused(["v0.2.127", "--server", serverDir("v0.2.127")], /announces 0\.2\.127, not older than 0\.2\.127/);
  });

  test("a carried manifest that does not verify against its archive", async () => {
    writeFileSync(join(filesDir, "v0.2.125", "Droppen.app.tar.gz"), "tampered archive");
    await refused(["v0.2.126", "--server", serverDir("v0.2.126")], /was refused: it does not verify/);
  });

  test("a release that exists already, even as a draft", async () => {
    const s = state();
    s.releases.unshift(release("v0.2.126", {}, { draft: true }));
    setState(s.releases, "v0.2.125");
    await refused(["v0.2.126", "--server", serverDir("v0.2.126")], /already has a release v0\.2\.126 \(a draft\)/);
  });

  test("the desktop app's manifest for another version, or another signature", async () => {
    await refused(
      ["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126", { announced: "0.2.125" })],
      /announces 0\.2\.125, not 0\.2\.126/,
    );
    const desktop = join(root, "desktop-0.2.126");
    writeFileSync(join(desktop, "updater", "Droppen.app.tar.gz.sig"), "SIG-other\n");
    writeFileSync(join(desktop, "updater", "latest.json"), manifest("0.2.126"));
    await refused(["v0.2.126", "--server", join(root, "server-v0.2.126"), "--desktop", desktop], /names another signature/);
  });

  test("a DMG without its notarisation ticket", async () => {
    await refused(
      ["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126", { dmg: "not notarised" })],
      /no stapled notarisation ticket: run desktop\/notarise\.sh/,
    );
  });

  test("a staged tarball that does not match SHA256SUMS, or a stray file", async () => {
    const server = serverDir("v0.2.126");
    writeFileSync(join(server, "droppen-v0.2.126-macos-universal.tar.gz"), "changed");
    await refused(["v0.2.126", "--server", server], /does not match SHA256SUMS/);
    const other = serverDir("v0.2.127");
    writeFileSync(join(other, "notes.txt"), "x");
    await refused(["v0.2.127", "--server", other], /files release-public\.sh does not make: notes\.txt/);
  });

  test("--desktop off a Mac, where the DMG's ticket cannot be checked", async () => {
    env.FAKE_UNAME = "Linux";
    await refused(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126")], /--desktop needs a Mac/);
  });

  test("the desktop app's latest.json that does not verify against its archive", async () => {
    const desktop = desktopDir("0.2.126");
    writeFileSync(join(desktop, "updater", "Droppen.app.tar.gz"), "tampered archive 0.2.126");
    await refused(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktop], /updater\/latest\.json was refused/);
  });

  test("an end-to-end test build as the desktop app", async () => {
    const desktop = desktopDir("0.2.126");
    writeFileSync(join(desktop, "E2E-TEST-BUILD.txt"), "An end-to-end test build.");
    await refused(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktop], /is an end-to-end test build: never published/);
  });

  test("a carried latest.json that announces no release version", async () => {
    writeFileSync(join(filesDir, "v0.2.125", "latest.json"), manifest("0.2.125-rc.1"));
    await refused(["v0.2.126", "--server", serverDir("v0.2.126")], /announces '0\.2\.125-rc\.1', not a release version/);
  });

  test("a latest-release lookup that fails for another reason than not found", async () => {
    env.FAKE_GH_LATEST_FAILS = "gh: Bad Gateway (HTTP 502)";
    await refused(["v0.2.126", "--server", serverDir("v0.2.126")], /cannot ask .* for its latest release: gh: Bad Gateway \(HTTP 502\)/);
  });

  test("contradictory options", async () => {
    const server = serverDir("v0.2.126");
    await refused(["v0.2.126", "--server", server, "--desktop", desktopDir("0.2.126"), "--no-manifest"], /brings its own latest\.json/);
    await refused(["v0.2.126-rc.1", "--server", server, "--desktop", join(root, "desktop-0.2.126")], /is a prerelease/);
    await refused(["0.2.126", "--server", server], /must look like v1\.2\.3/);
  });
});

describe("a key rotation", () => {
  test("--verify-pubkey checks latest.json with that key instead of tauri.conf.json's", async () => {
    const old = join(root, "old.key.pub");
    writeFileSync(old, "the key installed apps trust\n");
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126"), "--verify-pubkey", old, "--dry-run"]);
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /KEY ROTATION: latest\.json is checked with [0-9A-F]{16} \(.*old\.key\.pub\), the key installed apps trust, not tauri\.conf\.json's [0-9A-F]{16}/);
    const checks = readFileSync(env.FAKE_TOOL_LOG, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
      .filter((c) => c[0] === "check");
    assert.ok(checks.length > 0 && checks.every((c) => c[c.indexOf("--pubkey") + 1] === old), JSON.stringify(checks));
    const missing = await run(["v0.2.126", "--server", join(root, "server-v0.2.126"), "--verify-pubkey", join(root, "nope.pub")]);
    assert.notEqual(missing.code, 0);
    assert.match(missing.stderr, /--verify-pubkey .*nope\.pub: no such file/);
  });

  test("--verify-pubkey refuses the app's own key, even as a copy", async () => {
    const copy = join(root, "same.json");
    writeFileSync(copy, readFileSync(fileURLToPath(new URL("../desktop/src-tauri/tauri.conf.json", import.meta.url))));
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--verify-pubkey", copy, "--dry-run"]);
    assert.notEqual(r.code, 0, r.stdout);
    assert.match(r.stderr, /--verify-pubkey is tauri\.conf\.json's own key \([0-9A-F]{16}\): without a rotation, leave it out/);
    assert.deepEqual(writes(), []);
  });
});

describe("after publishing", () => {
  test("a latest release that does not serve the latest.json uploaded is said loudly", async () => {
    env.FAKE_CURL_STALE = "1";
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126")]);
    assert.notEqual(r.code, 0, r.stdout);
    assert.match(r.stderr, /v0\.2\.126 is published, but .* does not serve the latest\.json uploaded: look now/);
    assert.equal(state().latest, "v0.2.126", "published by then");
  });
});

describe("a draft that is not right is never published", () => {
  test("an upload that lost a file leaves the draft unpublished and the latest release as it was", async () => {
    env.FAKE_GH_DROP = "Droppen.app.tar.gz.sig";
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126")]);
    assert.notEqual(r.code, 0);
    assert.match(r.stderr, /does not hold exactly the files above/);
    assert.ok(!writes().some((c) => c[1] === "edit"), "never published");
    const s = state();
    assert.equal(s.latest, "v0.2.125");
    assert.equal(s.releases.find((x) => x.tag_name === "v0.2.126").draft, true);
  });
});

describe("what release.yml attests", () => {
  test("--sums-out writes the SHA256SUMS that is published, and --sums-expected holds the release to it", async () => {
    const server = serverDir("v0.2.126");
    const desktop = desktopDir("0.2.126");
    const sums = join(root, "SHA256SUMS.attested");
    const dry = await run(["v0.2.126", "--server", server, "--desktop", desktop, "--dry-run", "--sums-out", sums]);
    assert.equal(dry.code, 0, dry.stderr);
    assert.deepEqual(writes(), []);
    const r = await run(["v0.2.126", "--server", server, "--desktop", desktop, "--sums-expected", sums]);
    assert.equal(r.code, 0, r.stderr);
    assert.equal(readFileSync(join(filesDir, "v0.2.126", "SHA256SUMS"), "utf8"), readFileSync(sums, "utf8"));
    assert.match(readFileSync(sums, "utf8"), / {2}\.\/latest\.json\n$/);
  });

  test("a release whose SHA256SUMS is not the one expected writes nothing", async () => {
    const sums = join(root, "SHA256SUMS.attested");
    writeFileSync(sums, "something else\n");
    const r = await run(["v0.2.126", "--server", serverDir("v0.2.126"), "--desktop", desktopDir("0.2.126"), "--sums-expected", sums]);
    assert.notEqual(r.code, 0, r.stdout);
    assert.match(r.stderr, /is not .*SHA256SUMS\.attested byte for byte .*nothing was written/);
    assert.deepEqual(writes(), []);
  });

  test("--local-only checks the files and asks the repository nothing", async () => {
    // v0.2.125 is published already: a dry run of it can only check its files.
    const sums = join(root, "SHA256SUMS.local");
    const r = await run(["v0.2.125", "--server", serverDir("v0.2.125"), "--desktop", desktopDir("0.2.125"), "--dry-run", "--local-only", "--sums-out", sums]);
    assert.equal(r.code, 0, r.stderr);
    assert.deepEqual(calls(), []);
    assert.match(r.stdout, /was not asked anything, and nothing was written/);
    const listed = readFileSync(sums, "utf8").trim().split("\n").map((line) => line.split("  ./")[1]);
    assert.deepEqual(listed, [
      "droppen-v0.2.125-macos-universal.tar.gz",
      "remote-hand-v0.2.125-macos-universal.tar.gz",
      "Droppen-0.2.125.dmg",
      "Droppen.app.tar.gz",
      "Droppen.app.tar.gz.sig",
      "latest.json",
    ]);
    const bad = await run(["v0.2.125", "--server", join(root, "server-v0.2.125"), "--local-only"]);
    assert.notEqual(bad.code, 0);
    assert.match(bad.stderr, /--local-only is for a --dry-run/);
  });
});

describe("release.yml's jobs: the manifest tool only where nothing else is at stake", () => {
  const toolCalls = () => (existsSync(env.FAKE_TOOL_LOG) ? readFileSync(env.FAKE_TOOL_LOG, "utf8").trim().split("\n").filter(Boolean) : []);

  test("--sums-only writes the SHA256SUMS a checked dry run writes, runs no manifest tool and writes nothing", async () => {
    // Server-only: the carried latest.json and DMG are in it too.
    const server = serverDir("v0.2.126");
    const checked = join(root, "SHA256SUMS.checked");
    const dry = await run(["v0.2.126", "--server", server, "--dry-run", "--sums-out", checked]);
    assert.equal(dry.code, 0, dry.stderr);
    rmSync(env.FAKE_TOOL_LOG, { force: true });
    const only = join(root, "SHA256SUMS.only");
    const r = await run(["v0.2.126", "--server", server, "--sums-only", only]);
    assert.equal(r.code, 0, r.stderr);
    assert.deepEqual(toolCalls(), []);
    assert.deepEqual(writes(), []);
    assert.equal(readFileSync(only, "utf8"), readFileSync(checked, "utf8"));
    assert.match(readFileSync(only, "utf8"), /\.\/Droppen-0\.2\.125\.dmg\n.*\.\/latest\.json\n$/s);
  });

  test("--skip-manifest-check publishes without the tool, held to --sums-expected", async () => {
    const server = serverDir("v0.2.126");
    const desktop = desktopDir("0.2.126");
    const sums = join(root, "SHA256SUMS.attested");
    assert.equal((await run(["v0.2.126", "--server", server, "--desktop", desktop, "--sums-only", sums])).code, 0);
    rmSync(env.FAKE_TOOL_LOG, { force: true });
    const r = await run(["v0.2.126", "--server", server, "--desktop", desktop, "--skip-manifest-check", "--sums-expected", sums]);
    assert.equal(r.code, 0, r.stderr);
    assert.deepEqual(toolCalls(), []);
    assert.equal(state().latest, "v0.2.126");
    const rotation = await run(["v0.2.127", "--server", server, "--skip-manifest-check", "--verify-pubkey", sums]);
    assert.notEqual(rotation.code, 0);
    assert.match(rotation.stderr, /--verify-pubkey is a manifest check/);
  });
});

describe("publishing in the source repository", () => {
  test("the installer, the INSTALL texts and the attestation bundles go up in the draft; --verify-tag makes it for the existing tag", async () => {
    const server = serverDir("v0.2.126");
    for (const name of ["install.sh", "INSTALL-macos.txt", "INSTALL-linux.txt", "web-manifest.sigstore.json", "droppen-v0.2.126.sigstore.json"]) {
      writeFileSync(join(server, name), `${name}\n`);
    }
    const r = await run(["v0.2.126", "--server", server, "--desktop", desktopDir("0.2.126"), "--verify-tag"]);
    assert.equal(r.code, 0, r.stderr);
    const create = writes()[0];
    assert.ok(create.includes("--verify-tag") && create.includes("--draft"), JSON.stringify(create));
    const names = state().releases.find((x) => x.tag_name === "v0.2.126").assets.map((a) => a.name);
    for (const name of ["install.sh", "INSTALL-macos.txt", "INSTALL-linux.txt", "web-manifest.sigstore.json", "droppen-v0.2.126.sigstore.json"]) {
      assert.ok(names.includes(name), name);
    }
    assert.equal(names.at(-1), "latest.json");
    // Anything else is still refused.
    writeFileSync(join(server, "INSTALL-windows.txt"), "no\n");
    const stray = await run(["v0.2.126", "--server", server, "--dry-run"]);
    assert.notEqual(stray.code, 0);
    assert.match(stray.stderr, /holds files release-public\.sh does not make: INSTALL-windows\.txt/);
  });
});
