/*
 * Public pages take their font sizes from the theme's named scale
 * (docs/ui-spec.md §1): Tailwind's text-xs…text-5xl, never a fixed
 * `text-[13px]` and never the app's 11px `text-2xs`. The app's compact scale
 * is a separate, documented exception; this test guards the public surface.
 */
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../src/features/landing");

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(full);
    return /\.tsx?$/.test(entry.name) ? [full] : [];
  });
}

test("public pages take font sizes from the theme scale, never a fixed size or the app's text-2xs", () => {
  const files = sourceFiles(root);
  assert.ok(
    files.some((file) => file.endsWith(`${path.sep}landing.tsx`)),
    `expected ${path.join(root, "landing.tsx")} among the scanned files`,
  );
  const rules = [
    { pattern: /text-\[[0-9.]+(?:px|rem)\]/g, what: "a fixed px/rem size" },
    { pattern: /\btext-2xs\b/g, what: "the app's 11px token" },
  ];
  const offenders = files.flatMap((file) => {
    const source = readFileSync(file, "utf8");
    return rules.flatMap(({ pattern, what }) =>
      [...source.matchAll(pattern)].map((match) => `${path.relative(root, file)}: ${match[0]} (${what})`),
    );
  });
  assert.deepEqual(offenders, []);

  // `CapsLabel` defaults to the app's 11px `text-2xs`; a public page has to
  // say `text-xs` itself. This catches the default the rules above cannot see.
  const labels = files.flatMap((file) => {
    const source = readFileSync(file, "utf8");
    return [...source.matchAll(/<CapsLabel\b[^>]*>/g)]
      .filter((match) => !/\btext-xs\b/.test(match[0]))
      .map((match) => `${path.relative(root, file)}: ${match[0].slice(0, 80)} (CapsLabel without text-xs)`);
  });
  assert.deepEqual(labels, []);
});
