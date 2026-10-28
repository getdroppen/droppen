#!/usr/bin/env node
// A scripted `codex` for the demo. The Codex sessions in the tour are finished
// rollouts on disk (seed.mjs), so this only has to be a well-behaved CLI for
// whatever the server asks of one on the side: `--version`, and an App Server
// that answers every request without starting anything.

import readline from "node:readline";
import { CODEX_VERSION } from "../transcript.mjs";

const args = process.argv.slice(2);
if (args[0] === "--version" || args[0] === "-V") {
  console.log(`codex-cli ${CODEX_VERSION}`);
  process.exit(0);
}
if (args[0] !== "app-server") process.exit(0);

const input = readline.createInterface({ input: process.stdin });
for await (const raw of input) {
  let request;
  try {
    request = JSON.parse(raw);
  } catch {
    continue;
  }
  if (request.id === undefined) continue;
  const result = request.method === "initialize" ? { userAgent: `codex_cli_rs/${CODEX_VERSION}` } : {};
  process.stdout.write(JSON.stringify({ id: request.id, result }) + "\n");
}
