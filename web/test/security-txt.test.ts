// droppen.app's `/.well-known/security.txt` (RFC 9116): a static file of the
// web build (`public/.well-known/`), which Droppen Hub serves from the build
// like any other file, as `text/plain; charset=utf-8` (by its extension).
// It must say where to report (GitHub's private reporting and the mailbox
// SECURITY.md names), point at the policy, and not be expired — this test
// fails a month before `Expires`, so it is moved on in time.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const text = readFileSync(new URL("../public/.well-known/security.txt", import.meta.url), "utf8");
const fields = text
  .split("\n")
  .filter((line) => line && !line.startsWith("#"))
  .map((line) => {
    const at = line.indexOf(": ");
    assert.ok(at > 0, `"Field: value" lines only: ${line}`);
    return [line.slice(0, at), line.slice(at + 2)] as const;
  });
const all = (name: string) => fields.filter(([n]) => n === name).map(([, v]) => v);

test("security.txt names where to report, the policy and the languages, as RFC 9116 asks", () => {
  assert.ok(text.endsWith("\n") && !text.includes("\r"), "LF line ends, a final newline");
  assert.ok(/^[\x09\x0a\x20-\x7e]*$/.test(text), "plain ASCII");
  assert.deepEqual(all("Contact"), ["https://github.com/getdroppen/droppen/security/advisories/new", "mailto:security@droppen.app"]);
  const security = readFileSync(new URL("../../SECURITY.md", import.meta.url), "utf8");
  assert.ok(security.includes("security@droppen.app"), "the mailbox SECURITY.md names");
  assert.deepEqual(all("Policy"), ["https://github.com/getdroppen/droppen/blob/main/SECURITY.md"]);
  assert.deepEqual(all("Preferred-Languages"), ["en, zh"]);
  assert.deepEqual(all("Canonical"), ["https://droppen.app/.well-known/security.txt"]);
  for (const [name] of fields) assert.ok(["Contact", "Expires", "Policy", "Preferred-Languages", "Canonical"].includes(name), name);
});

test("security.txt has one Expires, within a year, and not about to pass", () => {
  const expires = all("Expires");
  assert.equal(expires.length, 1);
  assert.match(expires[0]!, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$/);
  const at = Date.parse(expires[0]!);
  const month = 30 * 24 * 60 * 60 * 1000;
  assert.ok(at - Date.now() > month, `Expires ${expires[0]} passes within a month: move it on (at most a year ahead) in web/public/.well-known/security.txt`);
  assert.ok(at - Date.now() < 13 * month, "no more than about a year ahead (RFC 9116 §2.5.5)");
});
