// Test the live audit's real CLI: `node --test scripts/ui-audit/run.test.mjs`.
// Set UI_AUDIT_CHANNEL=chrome when Playwright's bundled Chromium is not installed.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const script = fileURLToPath(new URL("./run.mjs", import.meta.url));

test("a hash URL is measured from the same top position as its first screenshot", { timeout: 30_000 }, async () => {
  const out = mkdtempSync(join(tmpdir(), "droppen-ui-audit-test-"));
  const server = createServer((_request, response) => {
    response.setHeader("Content-Type", "text/html; charset=utf-8");
    response.end(`<!doctype html>
      <html><head><title>Audit fixture</title></head>
      <body style="margin:0">
        <h1 style="margin:0">Audit top</h1>
        <div style="height:1600px"></div>
        <h2 id="target">Hash target</h2>
      </body></html>`);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");

  try {
    const address = server.address();
    assert.ok(address && typeof address === "object");
    await execFileAsync(process.execPath, [script, "--check", "--viewport", "mobile", "--url", `http://127.0.0.1:${address.port}/#target`, "--out", out]);
    const [result] = JSON.parse(readFileSync(join(out, "measurements.json"), "utf8"));
    const heading = result.controls.find((control) => control.tag === "H1" && control.text === "Audit top");
    assert.ok(heading, "the live audit measured the fixture's H1");
    assert.equal(heading.rect.y, 0, "measurements must be taken after the hash scroll is reset");
  } finally {
    server.close();
    await once(server, "close");
    rmSync(out, { recursive: true, force: true });
  }
});
