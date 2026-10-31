// The browsers the build writes for (build/target.ts) keep xterm.js working
// (#444): built for Vite's default ones, its answer to DECRQM threw, and the
// terminal stopped painting as soon as vim started.
// A fixture app built against the web app's own node_modules, then run.
import assert from "node:assert/strict";
import { mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import type { Terminal } from "@xterm/xterm";
import { build } from "vite";
import { BUILD_TARGET } from "../build/target.js";

const webRoot = resolve(import.meta.dirname, "..");

test("the built terminal answers what vim asks at startup", async () => {
  const root = mkdtempSync(join(tmpdir(), "droppen-target-"));
  symlinkSync(join(webRoot, "node_modules"), join(root, "node_modules"));
  writeFileSync(join(root, "entry.js"), 'import { Terminal } from "@xterm/xterm";\nglobalThis.builtTerminal = Terminal;\n');
  await build({
    root,
    configFile: false,
    logLevel: "silent",
    build: {
      target: BUILD_TARGET,
      outDir: join(root, "dist"),
      rollupOptions: { input: join(root, "entry.js"), output: { entryFileNames: "entry.js" } },
    },
  });
  await import(pathToFileURL(join(root, "dist", "entry.js")).href);
  const Built = (globalThis as { builtTerminal?: typeof Terminal }).builtTerminal;
  assert.ok(Built, "the build ran");

  const term = new Built({ cols: 80, rows: 24 });
  const answers: string[] = [];
  term.onData((data) => answers.push(data));
  // The cursor position, the device attributes and the cursor's blink mode
  // (DECRQM, `CSI ? 12 $ p`): what the terminal answers of vim's queries.
  await new Promise<void>((done) => term.write("\x1b[6n\x1b[>c\x1b[?12$p", done));
  assert.deepEqual(answers, ["\x1b[1;1R", "\x1b[>0;276;0c", "\x1b[?12;2$y"]);
  term.dispose();
});
