// The demo's sessions as this server's own: rows in the instance database's
// `owned_sessions` table (the server's `sessions::owned_sessions`), which is
// what lets it drive them rather than list them as external. The server makes
// the table when it first opens the database, so this waits for it, then
// inserts the ids seed.mjs wrote. Run as the server's user, while it runs.
//
//   node own.mjs --home /Users/dev --state /opt/demo/state
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { setTimeout as sleep } from "node:timers/promises";
import { DatabaseSync } from "node:sqlite";

const { values: opts } = parseArgs({ options: { home: { type: "string" }, state: { type: "string" } } });
if (!opts.home || !opts.state) {
  console.error("usage: own.mjs --home <dir> --state <dir>");
  process.exit(2);
}
const file = path.join(opts.home, ".local/share/remote-hand/instances/default/state.db");
const owned = JSON.parse(fs.readFileSync(path.join(opts.state, "owned.json"), "utf8"));

const deadline = Date.now() + 120_000;
for (;;) {
  if (fs.existsSync(file)) {
    const db = new DatabaseSync(file);
    try {
      db.exec("PRAGMA busy_timeout = 5000");
      const table = db.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'owned_sessions'").get();
      if (table) {
        const insert = db.prepare("INSERT OR IGNORE INTO owned_sessions (source, session_id) VALUES (?, ?)");
        let n = 0;
        for (const source of ["claude", "codex"]) for (const id of owned[source]) n += Number(insert.run(source, id).changes);
        console.log(`owned: ${n} sessions in ${file}`);
        break;
      }
    } finally {
      db.close();
    }
  }
  if (Date.now() > deadline) throw new Error(`timed out waiting for ${file}`);
  await sleep(500);
}
