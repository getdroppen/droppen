#!/usr/bin/env node
// The Rust half of THIRD_PARTY_NOTICES, rendered from cargo-about's JSON.
// Node >= 20, no dependencies.
//
//   cargo about generate --frozen --fail -c about.toml -m Cargo.toml --format json -o about.json
//   node scripts/notices/rust-notices.mjs about.json <out-file> [<what they are compiled into>]
//
// Several cargo-about reports, comma-separated (`server.json,app.json`), make
// one list: the desktop app's crates and its bundled server's, say, each crate
// and each licence text once. The third argument names what they are compiled
// into, for the heading (default: "the server").
//
// cargo-about has already refused any licence outside `about.toml`'s
// `accepted` list (`--fail`). This adds what its texts leave out:
//
// * a crate whose licence files cargo-about could not match (aws-lc-sys's
//   composite LICENSE, say) gets the canonical SPDX text from cargo-about, and
//   its own licence files are appended here in full as well;
// * every NOTICE file a crate ships (Apache-2.0 section 4(d)) is appended.

import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const LICENSE_FILE = /^(licen[cs]e|copying|copyright)([-._].*)?$/i;
const NOTICE_FILE = /^notice([-._].*)?$/i;

const [jsonArg, outArg, what = "the server"] = process.argv.slice(2);
if (!jsonArg || !outArg) {
  console.error("usage: node scripts/notices/rust-notices.mjs <about.json>[,<about.json>...] <out-file> [<what they are compiled into>]");
  process.exit(2);
}
const about = merge(jsonArg.split(",").map((file) => JSON.parse(readFileSync(file, "utf8"))));

/** One report of several: each crate once (by its cargo id), each licence text once, used by all who use it. */
function merge(reports) {
  const crates = new Map();
  const licenses = new Map();
  for (const report of reports) {
    for (const c of report.crates) if (!crates.has(c.package.id)) crates.set(c.package.id, c);
    for (const license of report.licenses) {
      const key = `${license.id}\u0000${license.text}`;
      const seen = licenses.get(key);
      if (!seen) {
        licenses.set(key, { ...license, used_by: [...license.used_by] });
        continue;
      }
      for (const use of license.used_by) {
        if (!seen.used_by.some((u) => u.crate.id === use.crate.id)) seen.used_by.push(use);
      }
    }
  }
  return { crates: [...crates.values()], licenses: [...licenses.values()] };
}
const label = (p) => `${p.name} ${p.version}`;
const filesIn = (dir, pattern) =>
  existsSync(dir)
    ? readdirSync(dir)
        .filter((name) => pattern.test(name) && statSync(join(dir, name)).isFile())
        .sort()
        .map((name) => ({ name, text: readFileSync(join(dir, name), "utf8").trimEnd() }))
    : [];

const rule = "-".repeat(80);
const crates = about.crates.map((c) => c.package).sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
const counts = new Map();
for (const c of about.crates) counts.set(c.license, (counts.get(c.license) ?? 0) + 1);

const lines = [
  `Rust crates compiled into ${what} (${crates.length})`,
  "",
  "Each crate with the licence expression it is used under:",
  "",
  ...about.crates
    .map((c) => `  ${label(c.package)}  ${c.license}`)
    .sort(),
  "",
  "The licence texts follow, each with the crates it covers.",
  "",
];
const synthesized = new Map(); // crate dir -> package
for (const license of about.licenses) {
  // MPL-2.0 (section 3.2) wants recipients told where the Source Code Form
  // is: each crate's repository, or its crates.io page when it names none.
  const mpl = /\bMPL-/.test(license.id);
  lines.push(rule, `${license.name} (${license.id})`, mpl ? "Used by (the source of each at the address beside it):" : "Used by:");
  for (const use of license.used_by) {
    const where = use.crate.repository || (mpl ? `https://crates.io/crates/${use.crate.name}/${use.crate.version}` : "");
    lines.push(`  ${label(use.crate)}${where ? `  ${where}` : ""}`);
    if (!license.source_path) synthesized.set(dirname(use.crate.manifest_path), use.crate);
  }
  lines.push("", license.text.trimEnd(), "");
}

// Crates whose own licence files were not matched get those files verbatim too.
const extras = [];
for (const [dir, pkg] of [...synthesized].sort((a, b) => (a[1].name < b[1].name ? -1 : 1))) {
  const files = filesIn(dir, LICENSE_FILE);
  if (files.length) extras.push({ pkg, kind: "licence files as the crate ships them", files });
}
for (const pkg of crates) {
  const files = filesIn(dirname(pkg.manifest_path), NOTICE_FILE);
  if (files.length) extras.push({ pkg, kind: "NOTICE", files });
}
if (extras.length) {
  lines.push(rule, "Files shipped by the crates themselves", "");
  for (const { pkg, kind, files } of extras) {
    lines.push(rule, `${label(pkg)}: ${kind}`, "");
    for (const f of files) lines.push(`[${f.name}]`, f.text, "");
  }
}
writeFileSync(outArg, lines.join("\n") + "\n");

console.error(`rust notices: ${crates.length} crates, ${about.licenses.length} licence texts`);
// Every licence id any crate names, beyond MIT and Apache-2.0, with who names it.
const named = new Map();
for (const c of about.crates) {
  for (const id of c.license.split(/[\s()]+/).filter((t) => t && !["AND", "OR", "WITH"].includes(t))) {
    if (id === "MIT" || id === "Apache-2.0") continue;
    if (!named.has(id)) named.set(id, []);
    named.get(id).push(c.package.name);
  }
}
for (const [id, names] of [...named].sort()) {
  console.error(`  ${id}: ${names.length} (${[...new Set(names)].sort().slice(0, 8).join(", ")}${names.length > 8 ? ", …" : ""})`);
}
console.error(`  canonical texts used for ${synthesized.size} crate(s) without matching licence files: ${[...synthesized.values()].map((p) => p.name).sort().join(", ")}`);
