// Tests for scripts/install.sh's download checks and scripts/install-key.sh:
// `node --test scripts/install.test.mjs`. Needs sh, curl, tar and openssl.
//
// install.sh is read as a library (its last line, `main "$@"`, left out) and
// its download step is run against a release served from 127.0.0.1 by this
// file: the checksums, their signature (by a key made here, pinned into a
// copy of the script), and a tarball whose `droppen` only prints a version.
// Nothing is installed: the step stops at the staged binary, under a
// throwaway HOME.

import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, beforeEach, describe, test } from "node:test";
import { fileURLToPath } from "node:url";

const INSTALL = fileURLToPath(new URL("./install.sh", import.meta.url));
const INSTALL_KEY = fileURLToPath(new URL("./install-key.sh", import.meta.url));
const VERSION = "v9.9.9";
const TARBALL = `droppen-${VERSION}-linux-x86_64.tar.gz`;
const sha256 = (data) => createHash("sha256").update(data).digest("hex");

const keyPair = () => generateKeyPairSync("ec", { namedCurve: "prime256v1" });
const pem = (publicKey) => publicKey.export({ type: "spki", format: "pem" }).trimEnd();

let root, server, base, files, release;

/** install.sh as a library, with `key` (a PEM, or "") as its SUMS_KEY. */
function library(key) {
  const text = readFileSync(INSTALL, "utf8");
  const lines = text.trimEnd().split("\n");
  assert.equal(lines.at(-1), 'main "$@"', "install.sh ends by running main");
  let body = `${lines.slice(0, -1).join("\n")}\n`;
  // The key install.sh pins (or ''), single-quoted: replaced by this one.
  const pinned = /\nSUMS_KEY='[^']*'\n/g;
  assert.equal(body.match(pinned)?.length, 1, "install.sh sets SUMS_KEY once");
  body = body.replace(pinned, `\nSUMS_KEY='${key}'\n`);
  const file = join(root, `install-${Math.random().toString(36).slice(2)}.sh`);
  writeFileSync(file, body);
  return file;
}

/** Runs install.sh's download step; `{code, stdout, stderr}`. */
function download(lib, env = {}) {
  const home = join(root, "home");
  const script = `. "$1"; set_paths; OS=linux; ARCH=x86_64; VERSION=${VERSION}; TMP=$(mktemp -d "$HOME/t.XXXXXX"); check_base; download_and_verify; echo "installed: $NEW_VERSION"`;
  return new Promise((resolve) => {
    execFile("sh", ["-c", script, "sh", lib], {
      env: { PATH: process.env.PATH, HOME: home, DROPPEN_RELEASE_BASE: base, ...env },
    }, (error, stdout, stderr) => resolve({ code: error ? error.code : 0, stdout, stderr }));
  });
}

/** The staged binary download_and_verify leaves, if any. */
const staged = () => join(root, "home", ".local/share/remote-hand/bin/.installer-new");

/** The release's files, served at /download/<version>/<name>. */
function publish(name, content) {
  writeFileSync(join(release, name), content);
}

function tarball() {
  const dir = join(root, "pkg");
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "droppen"), "#!/bin/sh\necho 'droppen 9.9.9'\n");
  chmodSync(join(dir, "droppen"), 0o755);
  execFileSync("tar", ["-czf", join(release, TARBALL), "-C", dir, "droppen"]);
  return readFileSync(join(release, TARBALL));
}

/** A signed release: SHA256SUMS, SHA256SUMS-server and its signature by `privateKey`. */
function signedRelease(privateKey, { sums } = {}) {
  const bytes = tarball();
  const list = sums ?? `${sha256(bytes)}  ./${TARBALL}\n`;
  publish("SHA256SUMS", `${list}${"0".repeat(64)}  ./Droppen-9.9.9.dmg\n`);
  publish("SHA256SUMS-server", list);
  publish("SHA256SUMS-server.sig", sign("sha256", Buffer.from(list), privateKey));
}

before(async () => {
  root = mkdtempSync(join(tmpdir(), "install-test-"));
  files = join(root, "served");
  server = createServer((req, res) => {
    const path = join(files, decodeURIComponent(new URL(req.url, "http://x").pathname));
    if (!path.startsWith(files) || !existsSync(path)) {
      res.writeHead(404).end("not found");
      return;
    }
    res.writeHead(200).end(readFileSync(path));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${server.address().port}/releases`;
});

after(() => {
  server.close();
  rmSync(root, { recursive: true, force: true });
});

beforeEach(() => {
  rmSync(files, { recursive: true, force: true });
  rmSync(join(root, "home"), { recursive: true, force: true });
  release = join(files, "releases", "download", VERSION);
  mkdirSync(release, { recursive: true });
  mkdirSync(join(root, "home"));
});

describe("install.sh with no release key pinned", () => {
  test("goes by the release's SHA256SUMS, as before", async () => {
    signedRelease(keyPair().privateKey);
    const r = await download(library(""));
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /SHA-256 verified: [0-9a-f]{64} \(droppen-v9\.9\.9-linux-x86_64\.tar\.gz\)/);
    assert.doesNotMatch(r.stdout, /Signature verified/);
    assert.match(r.stdout, /installed: droppen 9\.9\.9/);
  });
});

describe("install.sh with a release key pinned", () => {
  const { publicKey, privateKey } = keyPair();

  test("checks SHA256SUMS-server's signature, then the tarball against it", async () => {
    signedRelease(privateKey);
    const r = await download(library(pem(publicKey)));
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /Signature verified: SHA256SUMS-server of v9\.9\.9/);
    assert.match(r.stdout, /SHA-256 verified/);
    assert.match(r.stdout, /installed: droppen 9\.9\.9/);
  });

  test("refuses checksums changed after they were signed", async () => {
    signedRelease(privateKey);
    publish("SHA256SUMS-server", `${"1".repeat(64)}  ./${TARBALL}\n`);
    const r = await download(library(pem(publicKey)));
    assert.equal(r.code, 1);
    assert.match(r.stderr, /does not check out with the release key/);
    assert.ok(!existsSync(staged()), "nothing staged");
  });

  test("refuses a signature by another key", async () => {
    signedRelease(keyPair().privateKey);
    const r = await download(library(pem(publicKey)));
    assert.equal(r.code, 1);
    assert.match(r.stderr, /does not check out with the release key/);
  });

  test("refuses a tarball the signed checksums do not list as it is", async () => {
    signedRelease(privateKey, { sums: `${"2".repeat(64)}  ./${TARBALL}\n` });
    const r = await download(library(pem(publicKey)));
    assert.equal(r.code, 1);
    assert.match(r.stdout, /Signature verified/);
    assert.match(r.stderr, /checksum mismatch for droppen-v9\.9\.9-linux-x86_64\.tar\.gz/);
    assert.ok(!existsSync(staged()), "nothing staged");
  });

  test("the latest release without a signature is refused, with no way around it offered", async () => {
    signedRelease(privateKey);
    rmSync(join(release, "SHA256SUMS-server.sig"));
    const refused = await download(library(pem(publicKey)));
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /the release v9\.9\.9 has no signature over its checksums/);
    assert.doesNotMatch(refused.stderr, /DROPPEN_ALLOW_UNSIGNED/);
    assert.ok(!existsSync(staged()), "nothing staged");
  });

  test("a release named with DROPPEN_VERSION without a signature is refused, unless DROPPEN_ALLOW_UNSIGNED=1", async () => {
    signedRelease(privateKey);
    rmSync(join(release, "SHA256SUMS-server.sig"));
    const refused = await download(library(pem(publicKey)), { DROPPEN_VERSION: VERSION });
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /published before they were signed has none/);
    assert.match(refused.stderr, /DROPPEN_ALLOW_UNSIGNED=1/);
    assert.ok(!existsSync(staged()), "nothing staged");

    const allowed = await download(library(pem(publicKey)), { DROPPEN_VERSION: VERSION, DROPPEN_ALLOW_UNSIGNED: "1" });
    assert.equal(allowed.code, 0, allowed.stderr);
    assert.match(allowed.stderr, /going by its SHA256SUMS alone/);
    assert.match(allowed.stdout, /installed: droppen 9\.9\.9/);
  });

  test("DROPPEN_ALLOW_UNSIGNED=1 never overrides a signature that does not check out", async () => {
    signedRelease(keyPair().privateKey);
    const r = await download(library(pem(publicKey)), { DROPPEN_ALLOW_UNSIGNED: "1" });
    assert.equal(r.code, 1);
    assert.match(r.stderr, /does not check out with the release key/);
  });
});

describe("where releases may come from", () => {
  const check = (where) =>
    new Promise((resolve) => {
      execFile("sh", ["-c", '. "$1"; check_base && echo ok', "sh", library("")], {
        env: { PATH: process.env.PATH, HOME: join(root, "home"), DROPPEN_RELEASE_BASE: where },
      }, (error, stdout, stderr) => resolve({ code: error ? error.code : 0, stdout, stderr }));
    });

  for (const where of [
    "https://github.com/getdroppen/droppen/releases",
    "https://mirror.example/droppen",
    "http://127.0.0.1:8000/releases",
    "http://localhost/releases",
    "http://[::1]:9000/releases",
  ]) {
    test(`takes ${where}`, async () => {
      const r = await check(where);
      assert.equal(r.code, 0, r.stderr);
    });
  }

  for (const where of [
    "http://github.com/getdroppen/droppen/releases",
    "http://127.0.0.1.example/releases",
    "http://127.0.0.1:80@mirror.example/releases",
    "http://localhost.example/releases",
    "https://user@mirror.example/releases",
    "ftp://mirror.example/releases",
    "github.com/getdroppen/droppen/releases",
  ]) {
    test(`refuses ${where}`, async () => {
      const r = await check(where);
      assert.equal(r.code, 1);
      assert.match(r.stderr, /DROPPEN_RELEASE_BASE must be an https:\/\/ address/);
    });
  }
});

describe("scripts/install-key.sh", () => {
  const keyOf = (file) => {
    try {
      return { code: 0, out: execFileSync(INSTALL_KEY, [file], { stdio: ["ignore", "pipe", "pipe"] }).toString() };
    } catch (error) {
      return { code: error.status, out: error.stdout.toString(), err: error.stderr.toString() };
    }
  };
  const withKey = (assignment) => {
    const file = join(root, `key-${Math.random().toString(36).slice(2)}.sh`);
    writeFileSync(file, `#!/bin/sh\n# SUMS_KEY= in a comment is not it\n${assignment}\necho rest\n`);
    return file;
  };

  test("this repository's install.sh: the key the script itself uses, or none", () => {
    library("");
    const assigned = execFileSync("sh", ["-c", '. "$1"; printf %s "$SUMS_KEY"', "sh", libraryOfRepo()]).toString();
    const r = keyOf(INSTALL);
    assert.equal(r.code, 0, r.err);
    assert.equal(r.out.trimEnd(), assigned);
  });

  test("prints a pinned key as install.sh reads it", () => {
    const key = pem(keyPair().publicKey);
    const file = library(key);
    assert.deepEqual(keyOf(file), { code: 0, out: `${key}\n` });
    const assigned = execFileSync("sh", ["-c", '. "$1"; printf %s "$SUMS_KEY"', "sh", file]).toString();
    assert.equal(assigned, key);
  });

  test("refuses a key written any other way", () => {
    const key = pem(keyPair().publicKey);
    for (const assignment of [
      `SUMS_KEY="${key}"`,
      `SUMS_KEY='${key.replaceAll("\n", " ")}'`,
      `SUMS_KEY='${key}'; SUMS_KEY=''`,
      `SUMS_KEY='${key.replace("-----END PUBLIC KEY-----", "")}'`,
      `SUMS_KEY='${key}'\nSUMS_KEY=''`,
      "SUMS_KEY=",
    ]) {
      assert.equal(keyOf(withKey(assignment)).code, 1, assignment);
    }
  });

  test("an installer from before the key has none", () => {
    assert.deepEqual(keyOf(withKey("# no key at all")), { code: 0, out: "" });
  });
});

/** The repository's install.sh as a library, unchanged. */
function libraryOfRepo() {
  const lines = readFileSync(INSTALL, "utf8").trimEnd().split("\n");
  const file = join(root, "install-repo.sh");
  writeFileSync(file, `${lines.slice(0, -1).join("\n")}\n`);
  return file;
}

describe("install.sh's summary", () => {
  // The aligned "  Label:     text" lines it ends with, as written in the script.
  const summary = () =>
    readFileSync(INSTALL, "utf8")
      .split("\n")
      .filter((line) => /^\s*say "  [A-Z][A-Za-z ]*:\s{2,}/.test(line));

  test("is in English, naming the buttons as the English interface names them", () => {
    const lines = summary();
    assert.ok(lines.length >= 10, "the summary lines are found");
    for (const line of lines) assert.doesNotMatch(line, /[　-鿿＀-￯]/, `a CJK word in an English line: ${line.trim()}`);
    const next = lines.filter((line) => line.includes("Next:"));
    assert.equal(next.length, 1);
    // As the shell prints it.
    const printed = execFileSync("sh", ["-c", `say() { printf '%s\\n' "$*"; }; ${next[0].trim()}`]).toString();
    assert.equal(printed, '  Next:       choose "Sign in to Droppen" on that page to add this device to your Droppen account\n');
  });
});
