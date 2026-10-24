// Tests for scripts/release-notes.sh: `node --test scripts/release-notes.test.mjs`.
//
// The files' table of a release's page. release-notes.sh reads
// release-public.sh and install.sh beside it (or the install.sh
// DROPPEN_RELEASE_INSTALL_SH names), so each test runs it in a scratch copy of
// those scripts, with an install.sh of its own.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import { chmodSync, copyFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
const ARGS = ["v0.2.131", "0123456789abcdef0123456789abcdef01234567", "0123456789ab", "0.2.131", "1"];
const SIGNED_ROW =
  "| `SHA256SUMS-server`, `SHA256SUMS-server.sig` | the tarballs' checksums and their signature by the release key `install.sh` pins, which it checks before it installs anything |";

const scratch = [];
afterEach(() => {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** release-notes.sh and what it reads, with `install` as their install.sh. */
function scripts(install) {
  const dir = mkdtempSync(join(tmpdir(), "release-notes-test-"));
  scratch.push(dir);
  for (const name of ["release-notes.sh", "release-public.sh", "install-key.sh"]) {
    copyFileSync(join(HERE, name), join(dir, name));
    chmodSync(join(dir, name), 0o755);
  }
  writeFileSync(join(dir, "install.sh"), install);
  return dir;
}

/** What release-notes.sh in `dir` prints; never as on GitHub Actions. */
function notes(dir, env = {}) {
  return spawnSync("bash", [join(dir, "release-notes.sh"), ...ARGS], { encoding: "utf8", env: { PATH: process.env.PATH, ...env } });
}

const pinning = () => {
  const { publicKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
  return `#!/bin/sh\nSUMS_KEY='${publicKey.export({ type: "spki", format: "pem" }).trimEnd()}'\n`;
};
const table = (text) => text.split("\n").filter((line) => line.startsWith("| "));

test("a release whose install.sh pins a release key lists its signed checksums, after the installer", () => {
  const r = notes(scripts(pinning()));
  assert.equal(r.status, 0, r.stderr);
  const rows = table(r.stdout);
  const at = rows.indexOf(SIGNED_ROW);
  assert.notEqual(at, -1, rows.join("\n"));
  assert.match(rows[at - 1], /^\| `install\.sh` \|/);
  assert.match(rows[at + 1], /^\| `INSTALL-macos\.txt`, `INSTALL-linux\.txt` \|/);
});

test("while install.sh pins none, the table has no signed checksums and is otherwise the same", () => {
  const pinned = notes(scripts(pinning()));
  const unpinned = notes(scripts("#!/bin/sh\nSUMS_KEY=''\n"));
  assert.equal(unpinned.status, 0, unpinned.stderr);
  assert.doesNotMatch(unpinned.stdout, /SHA256SUMS-server/);
  assert.equal(unpinned.stdout, pinned.stdout.replace(`${SIGNED_ROW}\n`, ""));
});

test("the install.sh it is given, not the one beside it, decides the row", () => {
  const dir = scripts(pinning());
  const unpinned = join(dir, "staged-install.sh");
  writeFileSync(unpinned, "#!/bin/sh\nSUMS_KEY=''\n");
  const r = notes(dir, { DROPPEN_RELEASE_INSTALL_SH: unpinned });
  assert.equal(r.status, 0, r.stderr);
  assert.doesNotMatch(r.stdout, /SHA256SUMS-server/);
  const pinned = join(dir, "pinned-install.sh");
  writeFileSync(pinned, pinning());
  const other = notes(scripts("#!/bin/sh\nSUMS_KEY=''\n"), { DROPPEN_RELEASE_INSTALL_SH: pinned });
  assert.equal(other.status, 0, other.stderr);
  assert.ok(table(other.stdout).includes(SIGNED_ROW), other.stdout);
});

test("this source's install.sh is read as it is: its key decides the row", () => {
  const key = spawnSync(join(HERE, "install-key.sh"), [join(HERE, "install.sh")], { encoding: "utf8" });
  assert.equal(key.status, 0, key.stderr);
  const r = spawnSync("bash", [join(HERE, "release-notes.sh"), ...ARGS], { encoding: "utf8", env: { PATH: process.env.PATH } });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(table(r.stdout).includes(SIGNED_ROW), key.stdout !== "");
});

test("an install.sh whose key cannot be read stops it, rather than leaving the row out", () => {
  const r = notes(scripts("#!/bin/sh\nSUMS_KEY='not a key'\n"));
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /cannot read the release key/);
  assert.equal(r.stdout, "");
});
