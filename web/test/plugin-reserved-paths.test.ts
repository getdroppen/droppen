// The server refuses an install whose `droppen.aliases` names a top-level
// path the shell already answers (`docs/plugins/runtime.md` §1), and this
// test keeps that list in step with the router. The list used to live in
// `web/build/droppen-plugins.ts` (`RESERVED_PATHS`); with the build-time
// distribution gone it moves to the Rust side, next to the install checks.
//
// TODO(server): `server-rs/crates/droppen-plugin-sdk/reserved-paths.txt` is
// lane S's to add. Until it is here the test skips, and it starts checking
// the router automatically once the file lands.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";

const webRoot = resolve(import.meta.dirname, "..");
const reservedFile = resolve(webRoot, "..", "server-rs", "crates", "droppen-plugin-sdk", "reserved-paths.txt");

/**
 * The file's paths as a set, without leading slashes: one per line (blank
 * lines and `#` comments skipped), or a JSON array. The format is the Rust
 * side's; this reads either so the test is not a second copy of the list.
 */
function reservedPaths(text: string): Set<string> {
  const trimmed = text.trim();
  const entries = trimmed.startsWith("[")
    ? (JSON.parse(trimmed) as unknown[])
    : trimmed.split(/\r?\n/).map((line) => line.trim()).filter((line) => line && !line.startsWith("#"));
  return new Set(
    entries.map((entry) => {
      if (typeof entry !== "string") throw new Error(`${reservedFile}: every path must be a string`);
      if (!/^\/?[a-z0-9-]+$/.test(entry)) throw new Error(`${reservedFile}: ${JSON.stringify(entry)} is not a top-level path`);
      return entry.replace(/^\//, "");
    }),
  );
}

test(
  "no alias may take a path the shell's router already answers",
  { skip: !existsSync(reservedFile) && `TODO: ${reservedFile} does not exist yet (lane S adds it)` },
  () => {
    const reserved = reservedPaths(readFileSync(reservedFile, "utf8"));
    const router = readFileSync(resolve(webRoot, "src/app/router.tsx"), "utf8");
    const tops = [...router.matchAll(/getParentRoute: \(\) => rootRoute,\s*path: "([^"/$]+)/g)].map((m) => m[1]!);
    assert.ok(tops.length >= 8, `found the router's top-level routes: ${tops}`);
    for (const top of tops) assert.ok(reserved.has(top), `/${top} is reserved`);
  },
);
