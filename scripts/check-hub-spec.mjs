#!/usr/bin/env node
// Is the committed hub OpenAPI snapshot (`openapi/hub.json`) the deployed
// hub's document? The web app's generated types come from the snapshot; this
// is what keeps the snapshot from going stale between releases.
//
//   node scripts/check-hub-spec.mjs
//   HUB_OPENAPI_URL=... node scripts/check-hub-spec.mjs
//
// Exit 0: they match. 1: drift (the message names what changed). 2: the hub
// could not be read (network, outage) — not a contract finding.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const url = process.env.HUB_OPENAPI_URL ?? "https://api.droppen.app/openapi.json";
const file = fileURLToPath(new URL("../openapi/hub.json", import.meta.url));

/** JSON with object keys sorted, so formatting and order never count. */
function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted(value[key])]));
  }
  return value;
}
const canon = (value) => JSON.stringify(sorted(value));

let snapshot;
try {
  snapshot = JSON.parse(readFileSync(file, "utf8"));
} catch (error) {
  console.error(`check-hub-spec: cannot read ${file}: ${error.message}`);
  process.exit(2);
}

let live;
try {
  const response = await fetch(url, { signal: AbortSignal.timeout(20_000) });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  live = await response.json();
} catch (error) {
  console.error(`check-hub-spec: cannot read ${url}: ${error.message}`);
  process.exit(2);
}

if (canon(snapshot) === canon(live)) {
  console.log(`check-hub-spec: openapi/hub.json matches ${url}`);
  process.exit(0);
}

const section = (name) => [snapshot[name] ?? {}, live[name] ?? {}];
const describe = (a, b) => {
  const onlySnapshot = Object.keys(a).filter((key) => !(key in b));
  const onlyLive = Object.keys(b).filter((key) => !(key in a));
  const changed = Object.keys(a).filter((key) => key in b && canon(a[key]) !== canon(b[key]));
  return { onlySnapshot, onlyLive, changed };
};
const report = (label, a, b) => {
  const { onlySnapshot, onlyLive, changed } = describe(a, b);
  if (onlySnapshot.length) console.error(`  ${label} only in the snapshot: ${onlySnapshot.join(", ")}`);
  if (onlyLive.length) console.error(`  ${label} only on the hub: ${onlyLive.join(", ")}`);
  if (changed.length) console.error(`  ${label} changed: ${changed.join(", ")}`);
};

console.error(`check-hub-spec: openapi/hub.json is not the document ${url} serves`);
report("paths", ...section("paths"));
const [a, b] = section("components");
report("schemas", a?.schemas ?? {}, b?.schemas ?? {});
if (canon(snapshot.info) !== canon(live.info)) console.error("  info changed");
console.error("  refresh: curl -fsS " + url + " > openapi/hub.json && pnpm --filter @remote-hand/web gen:api:hub");
process.exit(1);
