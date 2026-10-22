#!/usr/bin/env node
// The hub API contract check (docs/hub-api/README.md). Node >= 20, no
// dependencies.
//
//   node docs/hub-api/check.mjs                 every hub route and control
//                                               message `t` the open clients
//                                               use is in index.json, and
//                                               index.json and the schemas agree
//   node docs/hub-api/check.mjs --list          also print everything found
//   node docs/hub-api/check.mjs --traffic F     also validate captured traffic
//                                               (DROPPEN_HUB_API_TRAFFIC JSON lines)
//
// Exits 1 on any finding.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const args = process.argv.slice(2);
const listAll = args.includes("--list");
const trafficAt = args.indexOf("--traffic");
const trafficFile = trafficAt >= 0 ? args[trafficAt + 1] : null;
if (trafficAt >= 0 && !trafficFile) {
  console.error("usage: node docs/hub-api/check.mjs [--list] [--traffic <file.jsonl>]");
  process.exit(2);
}

const index = JSON.parse(readFileSync(join(here, "index.json"), "utf8"));
const httpSchema = JSON.parse(readFileSync(join(here, "schema", "http.schema.json"), "utf8"));
const wsSchema = JSON.parse(readFileSync(join(here, "schema", "ws.schema.json"), "utf8"));
const failures = [];
const fail = (what) => failures.push(what);

// ------------------------------------------------------------ what is scanned

/**
 * Where the open clients live, and the hub's own end-to-end and stress runs
 * (`rh-hub-tools`, which speak the same API; absent from the public
 * repository, and a root that is not there is skipped).
 */
const ROOTS = [
  "web/src/lib/hub",
  "web/src/lib/e2e",
  "web/src/features/cloud",
  "server-rs/src/agent",
  "server-rs/src/aggregator",
  "server-rs/crates/rh-relay/src",
  "server-rs/crates/rh-client/src",
  "server-rs/crates/rh-devkit/src",
  "server-rs/crates/rh-hub-tools/src",
];

/**
 * Which channels each file speaks, and as which end: the first matching
 * prefix wins. A `t` found in a file must be documented on one of its
 * channels, in the direction the code uses it (sent or matched) from that end.
 */
const SPEAKS = [
  ["web/src/lib/hub/relay-link.ts", [["/v1/connect", "client"]]],
  ["web/src/lib/hub/", [["/v1/client", "client"]]],
  ["web/src/lib/e2e/", [["/v1/client", "client"], ["/v1/agent", "client"]]],
  ["web/src/features/cloud/turnstile.tsx", [["/turnstile", "client"]]],
  ["web/src/features/cloud/", [["/v1/client", "client"]]],
  ["server-rs/src/agent/relays.rs", [["/v1/agent", "client"], ["/v1/connect", "client"]]],
  ["server-rs/src/agent/", [["/v1/agent", "client"]]],
  ["server-rs/src/aggregator/gateway.rs", [["/api/agg/ws", "server"]]],
  ["server-rs/src/aggregator/live.rs", [["/api/agg/ws", "server"]]],
  ["server-rs/src/aggregator/", [["/v1/client", "client"]]],
  ["server-rs/crates/rh-relay/src/uplink.rs", [["/v1/relay", "client"]]],
  ["server-rs/crates/rh-relay/src/meter.rs", [["/v1/relay", "client"]]],
  ["server-rs/crates/rh-relay/src/lib.rs", [["/v1/relay", "client"]]],
  ["server-rs/crates/rh-relay/src/", [["/v1/connect", "server"]]],
  ["server-rs/crates/rh-client/src/", [["/v1/client", "client"], ["/v1/connect", "client"]]],
  ["server-rs/crates/rh-devkit/src/agent.rs", [["/v1/agent", "client"], ["/v1/connect", "client"]]],
  ["server-rs/crates/rh-devkit/src/", [["/v1/client", "client"], ["/v1/agent", "client"], ["/v1/connect", "client"]]],
  // The local gateway's e2e leg (the hub's white-box harness).
  ["server-rs/crates/rh-hub-tools/src/local_gateway.rs", [["/api/agg/ws", "client"]]],
  ["server-rs/crates/rh-hub-tools/src/", [["/v1/client", "client"], ["/v1/agent", "client"], ["/v1/connect", "client"]]],
];

/**
 * Channels of this computer's own server, not the hub's: the local gateway
 * (`GET /api/agg/ws`, docs/local-aggregator.md §6.3), which hands the other
 * devices' copies to this computer's own pages (the local app, a phone on the
 * LAN). Its frames are JSON tagged `t` like the hub's, some under the same
 * names (`sub`, `open`, `ping`), but the hub never sees them, so they are not
 * hub API and not in index.json. A file that speaks only these channels is
 * still scanned for hub routes; its `t` values are listed (`--list`) and not
 * looked up in index.json. In a file that also speaks a hub channel, every
 * `t` must be the hub's. SPEAKS names such files one by one, never by
 * directory, so the hub clients next to them (`aggregator/hub.rs`,
 * `copies.rs`, …) stay checked.
 */
const LOCAL = new Set(["/api/agg/ws"]);

/**
 * Not hub API, on purpose. Keep this short and say why for each.
 * `t: true`: every `t` value in the file is ignored (routes there are still
 * checked); `t: [..]`: only these.
 */
const IGNORE = [
  // The direct-path probe's own messages (probe <-> page): over a probe room
  // the hub relays without reading them (off unless DROPPEN_HUB_P2P_PROBE=1), and
  // over the DataChannel itself. The room's route is documented; its
  // messages are the probe's, not the hub's.
  { file: "server-rs/crates/rh-devkit/src/bin/rh-p2p-probe.rs", t: true },
  // A unit test's placeholder message for the relay's connection writer.
  { file: "server-rs/crates/rh-relay/src/conn.rs", t: ["x"] },
];

// ------------------------------------------------------------ a small lexer

const IDENT = /[A-Za-z_$]/;
const IDENT_REST = /[A-Za-z0-9_$]/;
const PUNCT3 = ["===", "!==", "..."];
const PUNCT2 = ["==", "!=", "=>", "::", "->", "&&", "||", "<=", ">=", "?.", ".."];
const REGEX_AFTER = new Set(["(", ",", "=", ":", "[", "!", "&", "|", "?", "{", "}", ";", "+", "-", "*", "%", "<", ">", "~", "^", "=>", "&&", "||", "==", "===", "!=", "!==", "<=", ">="]);
const REGEX_AFTER_WORDS = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "yield", "await", "instanceof"]);

/**
 * Tokens of a Rust or TypeScript source: `{k: "id"|"str"|"p"|"num", v, line}`,
 * comments dropped. A template literal is one `str` whose `${…}` parts read
 * `{}` (their own tokens follow it). Good enough for finding string
 * literals and a few token patterns; not a parser.
 */
function lex(src, lang) {
  const out = [];
  let i = 0;
  let line = 1;
  const n = src.length;
  const push = (k, v, at) => out.push({ k, v, line: at });
  const prevSig = () => out[out.length - 1];

  function readQuoted(q) {
    // at src[i] === q; returns the decoded-ish value (escapes kept simple)
    let v = "";
    i++;
    while (i < n && src[i] !== q) {
      if (src[i] === "\\") {
        const c = src[i + 1];
        if (c === "\n") line++;
        v += c === "n" ? "\n" : c === "t" ? "\t" : c;
        i += 2;
        continue;
      }
      if (src[i] === "\n") line++;
      v += src[i++];
    }
    i++;
    return v;
  }

  function readTemplate() {
    // at "`": the text with `${…}` as `{}`; the expressions' own tokens follow it
    const start = line;
    const inner = [];
    let v = "";
    i++;
    while (i < n && src[i] !== "`") {
      if (src[i] === "\\") {
        if (src[i + 1] === "\n") line++;
        v += src[i + 1];
        i += 2;
        continue;
      }
      if (src[i] === "$" && src[i + 1] === "{") {
        i += 2;
        const from = i;
        const fromLine = line;
        let depth = 0;
        // to the matching "}", stepping over strings and nested templates
        while (i < n) {
          const c = src[i];
          if (c === "{") depth++;
          else if (c === "}") {
            if (depth === 0) break;
            depth--;
          } else if (c === '"' || c === "'") {
            readQuoted(c);
            continue;
          } else if (c === "`") {
            readTemplate();
            continue;
          } else if (c === "\n") line++;
          i++;
        }
        inner.push(...lex(src.slice(from, i), lang).map((t) => ({ ...t, line: t.line + fromLine - 1 })));
        i++; // "}"
        v += "{}";
        continue;
      }
      if (src[i] === "\n") line++;
      v += src[i++];
    }
    i++;
    return [{ k: "str", v, line: start }, ...inner];
  }

  while (i < n) {
    const c = src[i];
    if (c === "\n") {
      line++;
      i++;
      continue;
    }
    if (c === " " || c === "\t" || c === "\r") {
      i++;
      continue;
    }
    // comments
    if (c === "/" && src[i + 1] === "/") {
      while (i < n && src[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && src[i + 1] === "*") {
      let depth = 0;
      while (i < n) {
        if (src[i] === "/" && src[i + 1] === "*") {
          depth++;
          i += 2;
          continue;
        }
        if (src[i] === "*" && src[i + 1] === "/") {
          depth--;
          i += 2;
          if (depth === 0 || lang === "ts") break;
          continue;
        }
        if (src[i] === "\n") line++;
        i++;
      }
      continue;
    }
    if (lang === "rs") {
      // raw and byte strings: r"…", r#"…"#, b"…", br#"…"#, c"…"
      const m = /^(b|c)?r(#*)"/.exec(src.slice(i, i + 40));
      if (m) {
        const hashes = m[2];
        const at = line;
        i += m[0].length;
        const end = src.indexOf('"' + hashes, i);
        const v = src.slice(i, end < 0 ? n : end);
        line += (v.match(/\n/g) || []).length;
        i = end < 0 ? n : end + 1 + hashes.length;
        push("str", v, at);
        continue;
      }
      if ((c === "b" || c === "c") && src[i + 1] === '"') {
        i++;
        const at = line;
        push("str", readQuoted('"'), at);
        continue;
      }
      if (c === "'" || (c === "b" && src[i + 1] === "'")) {
        // a char literal, or a lifetime
        const j = c === "b" ? i + 1 : i;
        if (src[j + 1] === "\\") {
          const close = src.indexOf("'", j + 3);
          i = close + 1;
          push("str", "", line);
          continue;
        }
        const cp = src.codePointAt(j + 1);
        const width = cp > 0xffff ? 2 : 1;
        if (src[j + 1 + width] === "'") {
          i = j + 2 + width;
          push("str", "", line);
          continue;
        }
        if (c === "'") {
          push("p", "'", line);
          i++;
          continue;
        }
      }
      if (c === "r" && src[i + 1] === "#" && IDENT.test(src[i + 2] || "")) {
        i += 2; // a raw identifier, r#type
        continue;
      }
    }
    if (c === '"') {
      const at = line;
      push("str", readQuoted('"'), at);
      continue;
    }
    if (lang === "ts" && c === "'") {
      // An apostrophe right after a word is prose in JSX text, not a string.
      if (i > 0 && IDENT_REST.test(src[i - 1])) {
        i++;
        continue;
      }
      const at = line;
      push("str", readQuoted("'"), at);
      continue;
    }
    if (lang === "ts" && c === "`") {
      out.push(...readTemplate());
      continue;
    }
    if (lang === "ts" && c === "/") {
      const p = prevSig();
      const regex = !p || (p.k === "p" && REGEX_AFTER.has(p.v)) || (p.k === "id" && REGEX_AFTER_WORDS.has(p.v));
      if (regex) {
        i++;
        let inClass = false;
        while (i < n && src[i] !== "\n") {
          if (src[i] === "\\") {
            i += 2;
            continue;
          }
          if (src[i] === "[") inClass = true;
          else if (src[i] === "]") inClass = false;
          else if (src[i] === "/" && !inClass) break;
          i++;
        }
        i++;
        while (i < n && /[a-z]/.test(src[i])) i++;
        push("re", "", line);
        continue;
      }
    }
    if (IDENT.test(c)) {
      let j = i + 1;
      while (j < n && IDENT_REST.test(src[j])) j++;
      push("id", src.slice(i, j), line);
      i = j;
      continue;
    }
    if (/[0-9]/.test(c)) {
      let j = i + 1;
      while (j < n && /[0-9A-Za-z_]/.test(src[j])) j++;
      if (src[j] === "." && /[0-9]/.test(src[j + 1] || "")) {
        j++;
        while (j < n && /[0-9A-Za-z_]/.test(src[j])) j++;
      }
      push("num", src.slice(i, j), line);
      i = j;
      continue;
    }
    const three = src.slice(i, i + 3);
    const two = src.slice(i, i + 2);
    if (PUNCT3.includes(three)) {
      push("p", three, line);
      i += 3;
    } else if (PUNCT2.includes(two)) {
      push("p", two, line);
      i += 2;
    } else {
      push("p", c, line);
      i++;
    }
  }
  return out;
}

// ------------------------------------------------------------ extraction

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const OPEN = { "(": ")", "[": "]", "{": "}" };
const CLOSE = { ")": "(", "]": "[", "}": "{" };
const is = (tok, k, v) => tok && tok.k === k && (v === undefined || tok.v === v);

/** Index of the bracket matching the opener at `at`. */
function matching(toks, at) {
  const open = toks[at].v;
  const close = OPEN[open];
  let depth = 0;
  for (let j = at; j < toks.length; j++) {
    if (toks[j].k !== "p") continue;
    if (toks[j].v === open) depth++;
    else if (toks[j].v === close && --depth === 0) return j;
  }
  return toks.length - 1;
}

/** The unmatched opener before `at`, walking back. */
function enclosing(toks, at) {
  let depth = 0;
  for (let j = at - 1; j >= 0; j--) {
    const t = toks[j];
    if (t.k !== "p") continue;
    if (CLOSE[t.v]) depth++;
    else if (OPEN[t.v]) {
      if (depth === 0) return j;
      depth--;
    }
  }
  return -1;
}

/** The HTTP method a path literal at `at` is used with, when the code says. */
function methodOf(toks, at) {
  let pos = at;
  for (let level = 0; level < 3; level++) {
    const open = enclosing(toks, pos);
    if (open < 0 || toks[open].v !== "(") return null;
    // `call("POST", "/v1/…")`: the method as the first argument
    const first = toks[open + 1];
    if (is(first, "str") && METHODS.includes(first.v) && is(toks[open + 2], "p", ",")) return first.v;
    let before = open - 1;
    // `call<T>(…)`: skip a generic argument list
    if (is(toks[before], "p", ">")) {
      let depth = 0;
      for (; before >= 0; before--) {
        if (is(toks[before], "p", ">")) depth++;
        else if (is(toks[before], "p", "<") && --depth === 0) {
          before--;
          break;
        }
      }
    }
    const callee = toks[before];
    if (is(callee, "id") && METHODS.includes(callee.v.toUpperCase())) return callee.v.toUpperCase();
    // `format!(…)` and `&format!(…)`: look one level out
    if (is(callee, "p", "!") && is(toks[before - 1], "id", "format")) {
      pos = before - 1;
      continue;
    }
    return null;
  }
  return null;
}

/** Hub paths in a string literal: `/v1/…`, query dropped, `{…}` segments as parameters. */
function pathsIn(text) {
  const found = [];
  for (const m of text.matchAll(/\/v1\/[A-Za-z0-9_.~\-{}*/]*/g)) {
    let path = m[0].replace(/\/+$/, "");
    if (path === "/v1" || path.length <= 4) continue;
    found.push(path);
  }
  return found;
}

const snake = (name) => name.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();

/** Token ranges of `#[cfg(test)]` items (Rust): tests build the other end's messages. */
function testRanges(toks) {
  const ranges = [];
  for (let j = 0; j + 6 < toks.length; j++) {
    if (is(toks[j], "p", "#") && is(toks[j + 1], "p", "[") && is(toks[j + 2], "id", "cfg") && is(toks[j + 3], "p", "(") && is(toks[j + 4], "id", "test")) {
      let k = j + 5;
      while (k < toks.length && !is(toks[k], "p", "{") && !is(toks[k], "p", ";")) k++;
      if (is(toks[k], "p", "{")) ranges.push([j, matching(toks, k)]);
    }
  }
  return (at) => ranges.some(([a, b]) => at >= a && at <= b);
}

/** Whether `toks` (Rust) are `#[cfg(test)]` (`inner`: `#![cfg(test)]`) at `j`; the index after it, or -1. */
function cfgTestAt(toks, j, inner) {
  let k = j;
  if (!is(toks[k++], "p", "#")) return -1;
  if (inner && !is(toks[k++], "p", "!")) return -1;
  const want = [["p", "["], ["id", "cfg"], ["p", "("], ["id", "test"], ["p", ")"], ["p", "]"]];
  for (const [kind, v] of want) if (!is(toks[k++], kind, v)) return -1;
  return k;
}

/**
 * The Rust files `toks` (of the file at `full`) declare test-only:
 * `#[cfg(test)] mod name;` — that module's `name.rs` or `name/mod.rs`. Such
 * a file is a test as a whole, like an inline `#[cfg(test)]` block (its
 * messages are the other end's, direction unknown), and so is one that says
 * `#![cfg(test)]` itself.
 */
function testModules(full, toks) {
  const out = [];
  for (let j = 0; j < toks.length; j++) {
    let k = cfgTestAt(toks, j, false);
    if (k < 0) continue;
    if (is(toks[k], "id", "pub")) {
      k++;
      if (is(toks[k], "p", "(")) k = matching(toks, k) + 1;
    }
    if (!(is(toks[k], "id", "mod") && is(toks[k + 1], "id") && is(toks[k + 2], "p", ";"))) continue;
    const name = toks[k + 1].v;
    const file = full.split(/[\\/]/).pop();
    // A module's children sit beside `mod.rs` / `lib.rs` / `main.rs`, and in
    // `foo/` for any other `foo.rs`.
    const dir = ["mod.rs", "lib.rs", "main.rs"].includes(file) ? dirname(full) : join(dirname(full), file.replace(/\.rs$/, ""));
    out.push(join(dir, `${name}.rs`), join(dir, name, "mod.rs"));
  }
  return out;
}

/**
 * Everything one file uses: `{routes: [{path, method, line}], ts: [{t, dir, line}]}`,
 * dir being "send", "recv" or "any" (unknown). `test`: the file is test code
 * as a whole ([`testModules`]).
 */
function extract(src, lang, test = false) {
  const toks = lex(src, lang);
  const routes = [];
  const ts = [];
  const wholeTest = lang === "rs" && (test || toks.some((_, j) => cfgTestAt(toks, j, true) >= 0));
  const inTest = wholeTest ? () => true : lang === "rs" ? testRanges(toks) : () => false;
  const t = (value, dir, at) => ts.push({ t: value, dir: inTest(at) ? "any" : dir, line: toks[at].line });

  for (let j = 0; j < toks.length; j++) {
    const tok = toks[j];
    if (tok.k === "str") {
      for (const path of pathsIn(tok.v)) routes.push({ path, method: methodOf(toks, j), line: tok.line });
      // JSON text inside a string (a test's canned message): direction unknown
      for (const m of tok.v.matchAll(/"t"\s*:\s*"([A-Za-z0-9_-]+)"/g)) ts.push({ t: m[1], dir: "any", line: tok.line });
    }
    const prev = toks[j - 1];
    const objStart = is(prev, "p", "{") || is(prev, "p", ",");
    // { t: "x" } (TS object, Rust struct literal) and json!({"t": "x"})
    if (objStart && (is(tok, "id", "t") || is(tok, "str", "t")) && is(toks[j + 1], "p", ":") && is(toks[j + 2], "str")) {
      t(toks[j + 2].v, "send", j);
    }
    if (lang === "ts") {
      // x.t === "y", x.t !== "y"
      if (is(tok, "p", ".") && is(toks[j + 1], "id", "t") && toks[j + 2] && ["===", "!==", "==", "!="].includes(toks[j + 2].v) && is(toks[j + 3], "str")) {
        t(toks[j + 3].v, "recv", j + 3);
      }
      // switch (x.t) { case "y": … }
      if (is(tok, "id", "switch") && is(toks[j + 1], "p", "(")) {
        const close = matching(toks, j + 1);
        if (is(toks[close - 1], "id", "t") && is(toks[close - 2], "p", ".") && is(toks[close + 1], "p", "{")) {
          const end = matching(toks, close + 1);
          let depth = 0;
          for (let k = close + 1; k <= end; k++) {
            if (is(toks[k], "p", "{")) depth++;
            else if (is(toks[k], "p", "}")) depth--;
            else if (depth === 1 && is(toks[k], "id", "case") && is(toks[k + 1], "str")) t(toks[k + 1].v, "recv", k + 1);
          }
        }
      }
    }
    if (lang === "rs") {
      // x["t"] == "y", x["t"].as_str() != Some("y")
      if (is(tok, "p", "[") && is(toks[j + 1], "str", "t") && is(toks[j + 2], "p", "]")) {
        let k = j + 3;
        if (is(toks[k], "p", ".") && is(toks[k + 1], "id", "as_str") && is(toks[k + 2], "p", "(") && is(toks[k + 3], "p", ")")) k += 4;
        if (toks[k] && (toks[k].v === "==" || toks[k].v === "!=")) {
          if (is(toks[k + 1], "str")) t(toks[k + 1].v, "recv", k + 1);
          else if (is(toks[k + 1], "id", "Some") && is(toks[k + 2], "p", "(") && is(toks[k + 3], "str")) t(toks[k + 3].v, "recv", k + 3);
        }
      }
      // match x["t"].as_str() { Some("a" | "b") => …, "c" => … }
      if (is(tok, "id", "match")) {
        let k = j + 1;
        let mentionsT = false;
        let depth = 0;
        for (; k < toks.length; k++) {
          const x = toks[k];
          if (x.k === "p" && (x.v === "(" || x.v === "[")) depth++;
          else if (x.k === "p" && (x.v === ")" || x.v === "]")) depth--;
          else if (depth === 0 && is(x, "p", "{")) break;
          else if (depth === 0 && is(x, "p", ";")) break;
          if (is(x, "str", "t") && (is(toks[k - 1], "p", "[") || (is(toks[k - 1], "p", "(") && is(toks[k - 2], "id", "get")))) mentionsT = true;
        }
        if (mentionsT && is(toks[k], "p", "{")) {
          const end = matching(toks, k);
          let d = 0;
          for (let a = k; a <= end; a++) {
            if (is(toks[a], "p", "{") || is(toks[a], "p", "(") || is(toks[a], "p", "[")) d++;
            else if (is(toks[a], "p", "}") || is(toks[a], "p", ")") || is(toks[a], "p", "]")) d--;
            if (d !== 1) continue;
            // an arm's pattern starts after "{" or "," (or "}" closing the previous arm's block)
            const startOfArm = is(toks[a - 1], "p", "{") || is(toks[a - 1], "p", ",") || is(toks[a - 1], "p", "}");
            if (!startOfArm) continue;
            let b = a;
            const values = [];
            if (is(toks[b], "id", "Some") && is(toks[b + 1], "p", "(")) {
              const close = matching(toks, b + 1);
              for (let c = b + 2; c < close; c++) if (is(toks[c], "str")) values.push(c);
              b = close + 1;
            } else {
              while (is(toks[b], "str")) {
                values.push(b);
                b += is(toks[b + 1], "p", "|") ? 2 : 1;
              }
            }
            if (values.length && (is(toks[b], "p", "=>") || is(toks[b], "id", "if"))) for (const v of values) t(toks[v].v, "recv", v);
          }
        }
      }
      // #[derive(Serialize|Deserialize)] #[serde(tag = "t", rename_all = "snake_case")] enum E { A {…}, B, … }
      if (is(tok, "id", "tag") && is(toks[j + 1], "p", "=") && is(toks[j + 2], "str", "t")) {
        let k = j;
        while (k < toks.length && !is(toks[k], "id", "enum")) k++;
        let derive = "";
        for (let b = j; b > Math.max(0, j - 40); b--) {
          if (is(toks[b], "id", "Serialize")) derive += "S";
          if (is(toks[b], "id", "Deserialize")) derive += "D";
        }
        const dir = derive.includes("S") && derive.includes("D") ? "any" : derive.includes("D") ? "recv" : "send";
        const open = k + 2;
        if (is(toks[open], "p", "{")) {
          const end = matching(toks, open);
          let d = 0;
          let rename = null;
          for (let a = open; a <= end; a++) {
            if (is(toks[a], "p", "{") || is(toks[a], "p", "(") || is(toks[a], "p", "[")) d++;
            else if (is(toks[a], "p", "}") || is(toks[a], "p", ")") || is(toks[a], "p", "]")) d--;
            if (d === 2 && is(toks[a], "id", "rename") && is(toks[a + 1], "p", "=") && is(toks[a + 2], "str")) rename = toks[a + 2].v;
            if (d === 1 && is(toks[a], "id") && /^[A-Z]/.test(toks[a].v) && (is(toks[a - 1], "p", "{") || is(toks[a - 1], "p", ",") || is(toks[a - 1], "p", "]"))) {
              t(rename ?? snake(toks[a].v), dir, a);
              rename = null;
            }
          }
        }
      }
    }
  }
  return { routes, ts };
}

// ------------------------------------------------------------ the index

const routes = index.routes.map((r) => ({ ...r, segments: r.path.split("/").slice(1) }));

/** Whether template segments match path segments (`{}` parameters in the path match template parameters only). */
function templateMatches(template, segments) {
  for (let i = 0; i < template.length; i++) {
    const t = template[i];
    if (t.startsWith("{*")) return segments.length > i;
    const s = segments[i];
    if (s === undefined) return false;
    if (t.startsWith("{")) continue;
    if (s.includes("{") || s !== t) return false;
  }
  return segments.length === template.length;
}

const routesFor = (path) => routes.filter((r) => templateMatches(r.segments, path.split("/").slice(1)));
const shortName = (channel) => channel.replace(/^\/v1\//, "").replace(/^\//, "");
const dirOf = (role, use) => (use === "any" ? null : (role === "client") === (use === "send") ? "c2s" : "s2c");

// ------------------------------------------------------------ scan

function walk(dir, out) {
  let names;
  try {
    names = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of names) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(rs|ts|tsx)$/.test(name) && !/\.(test|spec)\.tsx?$/.test(name) && !name.endsWith(".d.ts")) out.push(full);
  }
  return out;
}

const found = { routes: new Map(), ts: new Map(), local: new Map() };
let filesScanned = 0;
// Test-only Rust files, from every `#[cfg(test)] mod name;` first (the
// declaring file may come after the module's in the walk).
const testFiles = new Set();
for (const root of ROOTS) {
  for (const full of walk(join(repo, root), [])) {
    if (full.endsWith(".rs")) for (const t of testModules(full, lex(readFileSync(full, "utf8"), "rs"))) testFiles.add(t);
  }
}
for (const root of ROOTS) {
  for (const full of walk(join(repo, root), [])) {
    const file = relative(repo, full).split("\\").join("/");
    filesScanned++;
    const lang = file.endsWith(".rs") ? "rs" : "ts";
    const { routes: usedRoutes, ts: usedTs } = extract(readFileSync(full, "utf8"), lang, testFiles.has(full));
    const speaks = (SPEAKS.find(([prefix]) => file.startsWith(prefix)) ?? [null, []])[1];
    const hubSpeaks = speaks.filter(([channel]) => !LOCAL.has(channel));
    const onlyLocal = speaks.length > 0 && hubSpeaks.length === 0;
    const ignored = IGNORE.find((x) => x.file === file);

    for (const use of usedRoutes) {
      const where = `${file}:${use.line}`;
      const key = `${use.method ?? "*"} ${use.path}`;
      if (!found.routes.has(key)) found.routes.set(key, []);
      found.routes.get(key).push(where);
      const candidates = routesFor(use.path);
      if (!candidates.length) fail(`route ${use.path} (${where}) is not in index.json`);
      else if (use.method && !candidates.some((r) => r.method === use.method)) {
        fail(`route ${use.method} ${use.path} (${where}): index.json has it only as ${[...new Set(candidates.map((r) => r.method))].join(", ")}`);
      }
    }
    if (ignored?.t === true) continue;
    for (const use of usedTs) {
      if (Array.isArray(ignored?.t) && ignored.t.includes(use.t)) continue;
      const where = `${file}:${use.line}`;
      const key = `${use.t}`;
      const into = onlyLocal ? found.local : found.ts;
      if (!into.has(key)) into.set(key, []);
      into.get(key).push(`${where} (${use.dir}; ${speaks.map(([c]) => c).join(", ") || "no channel"})`);
      if (onlyLocal) continue;
      const ok = hubSpeaks.some(([channel, role]) => {
        const c = index.channels[channel];
        if (!c) return false;
        const dir = dirOf(role, use.dir);
        return dir ? c[dir].includes(use.t) : c.c2s.includes(use.t) || c.s2c.includes(use.t);
      });
      if (!ok) {
        const dirs = hubSpeaks.map(([channel, role]) => `${channel} ${dirOf(role, use.dir) ?? "c2s|s2c"}`).join(" or ");
        fail(`control message t="${use.t}" (${use.dir} at ${where}) is not in index.json under ${dirs || "any channel this file speaks (add it to SPEAKS)"}`);
      }
    }
  }
}

// ------------------------------------------------------------ JSON Schema (the subset used)

const KEYWORDS = new Set([
  "$schema", "$id", "$defs", "$ref", "$comment", "title", "description", "examples", "default",
  "type", "properties", "required", "additionalProperties", "items", "enum", "const",
  "pattern", "minLength", "maxLength", "minimum", "maximum", "minItems", "maxItems", "oneOf", "anyOf", "allOf",
]);

/** Refuse a schema that uses a keyword this validator would silently skip. */
function checkKeywords(schema, where) {
  if (typeof schema === "boolean") return;
  if (typeof schema !== "object" || schema === null) return fail(`${where}: not a schema`);
  for (const k of Object.keys(schema)) if (!KEYWORDS.has(k)) fail(`${where}: the keyword "${k}" is not supported by check.mjs`);
  for (const [k, v] of Object.entries(schema.$defs ?? {})) checkKeywords(v, `${where}/$defs/${k}`);
  for (const [k, v] of Object.entries(schema.properties ?? {})) checkKeywords(v, `${where}/properties/${k}`);
  if (typeof schema.additionalProperties === "object") checkKeywords(schema.additionalProperties, `${where}/additionalProperties`);
  if (schema.items !== undefined) checkKeywords(schema.items, `${where}/items`);
  for (const k of ["oneOf", "anyOf", "allOf"]) (schema[k] ?? []).forEach((s, i) => checkKeywords(s, `${where}/${k}/${i}`));
  if (schema.$ref !== undefined && resolve(schema.$ref, where) === undefined) fail(`${where}: $ref ${schema.$ref} does not resolve`);
}

/** The schema whose `#/…` references are being resolved. */
let currentRoot = null;
function resolve(ref, where) {
  if (!ref.startsWith("#/")) {
    fail(`${where}: only local $refs are supported (${ref})`);
    return undefined;
  }
  let node = currentRoot;
  for (const raw of ref.slice(2).split("/")) {
    const part = decodeURIComponent(raw).replace(/~1/g, "/").replace(/~0/g, "~");
    node = node?.[part];
  }
  return node;
}

const typeOf = (v) => (v === null ? "null" : Array.isArray(v) ? "array" : Number.isInteger(v) ? "integer" : typeof v);
function hasType(v, type) {
  const actual = typeOf(v);
  return actual === type || (type === "number" && actual === "integer");
}
function equal(a, b) {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every((k) => equal(a[k], b[k]));
}

/** Errors of `value` against `schema` (in `currentRoot`), as `path: why` lines. */
function validate(schema, value, path = "") {
  const errors = [];
  if (schema === true) return errors;
  if (schema === false) return [`${path || "/"}: nothing is allowed here`];
  if (schema.$ref !== undefined) errors.push(...validate(resolve(schema.$ref, path), value, path));
  if (schema.type !== undefined) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    if (!types.some((t) => hasType(value, t))) return [...errors, `${path || "/"}: ${typeOf(value)}, expected ${types.join(" or ")}`];
  }
  if ("const" in schema && !equal(value, schema.const)) errors.push(`${path || "/"}: ${JSON.stringify(value)}, expected ${JSON.stringify(schema.const)}`);
  if (schema.enum && !schema.enum.some((e) => equal(e, value))) errors.push(`${path || "/"}: ${JSON.stringify(value)}, expected one of ${JSON.stringify(schema.enum)}`);
  if (typeof value === "string") {
    const len = [...value].length;
    if (schema.minLength !== undefined && len < schema.minLength) errors.push(`${path}: shorter than ${schema.minLength}`);
    if (schema.maxLength !== undefined && len > schema.maxLength) errors.push(`${path}: longer than ${schema.maxLength}`);
    if (schema.pattern !== undefined && !new RegExp(schema.pattern, "u").test(value)) errors.push(`${path}: does not match ${schema.pattern}`);
  }
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) errors.push(`${path}: ${value} < ${schema.minimum}`);
    if (schema.maximum !== undefined && value > schema.maximum) errors.push(`${path}: ${value} > ${schema.maximum}`);
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) errors.push(`${path}: fewer than ${schema.minItems} items`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) errors.push(`${path}: more than ${schema.maxItems} items`);
    if (schema.items !== undefined) value.forEach((v, i) => errors.push(...validate(schema.items, v, `${path}/${i}`)));
  }
  if (typeOf(value) === "object") {
    for (const k of schema.required ?? []) if (!(k in value)) errors.push(`${path}/${k}: missing`);
    const props = schema.properties ?? {};
    for (const [k, v] of Object.entries(value)) {
      if (k in props) errors.push(...validate(props[k], v, `${path}/${k}`));
      else if (schema.additionalProperties !== undefined) errors.push(...validate(schema.additionalProperties, v, `${path}/${k}`));
    }
  }
  for (const s of schema.allOf ?? []) errors.push(...validate(s, value, path));
  if (schema.anyOf && !schema.anyOf.some((s) => validate(s, value, path).length === 0)) errors.push(`${path || "/"}: matches none of anyOf`);
  if (schema.oneOf) {
    const passing = schema.oneOf.filter((s) => validate(s, value, path).length === 0).length;
    if (passing !== 1) errors.push(`${path || "/"}: matches ${passing} of oneOf, expected exactly 1`);
  }
  return errors;
}

function withRoot(root, f) {
  const before = currentRoot;
  currentRoot = root;
  try {
    return f();
  } finally {
    currentRoot = before;
  }
}

// ------------------------------------------------------------ index <-> schemas

withRoot(httpSchema, () => checkKeywords(httpSchema, "http.schema.json"));
withRoot(wsSchema, () => checkKeywords(wsSchema, "ws.schema.json"));

const routeKey = (r) => `${r.method} ${r.path}`;
const NO_JSON = new Set(["text", "html", "binary", "none"]);
for (const r of routes) {
  const defs = httpSchema.$defs[routeKey(r)]?.$defs ?? {};
  if (r.websocket) continue;
  if (!NO_JSON.has(r.response ?? "json") && !defs.response) fail(`http.schema.json: no response schema for ${routeKey(r)}`);
  if (["POST", "PUT", "PATCH"].includes(r.method) && r.response !== "none" && !defs.request) fail(`http.schema.json: no request schema for ${routeKey(r)}`);
}
for (const key of Object.keys(httpSchema.$defs)) {
  if (/^[A-Z]+ \//.test(key) && !routes.some((r) => routeKey(r) === key)) fail(`http.schema.json: ${key} is not a route in index.json`);
}
for (const [channel, c] of Object.entries(index.channels)) {
  for (const dir of ["c2s", "s2c"]) {
    const top = wsSchema.$defs[`${channel} ${dir}`];
    if (!top) {
      fail(`ws.schema.json: no "${channel} ${dir}"`);
      continue;
    }
    const refs = (top.oneOf ?? []).map((s) => s.$ref);
    const listed = new Set(top.properties?.t?.enum ?? []);
    for (const t of c[dir]) {
      const name = `${shortName(channel)}.${dir}.${t}`;
      if (!wsSchema.$defs[name]) fail(`ws.schema.json: no ${name}`);
      if (!refs.includes(`#/$defs/${name}`)) fail(`ws.schema.json: "${channel} ${dir}" does not list ${name}`);
      if (!listed.has(t)) fail(`ws.schema.json: "${channel} ${dir}" t enum lacks ${t}`);
    }
    for (const t of listed) if (!c[dir].includes(t)) fail(`ws.schema.json: "${channel} ${dir}" has ${t}, index.json does not`);
  }
}
for (const key of Object.keys(wsSchema.$defs)) {
  const m = /^([a-z]+)\.(c2s|s2c)\.(.+)$/.exec(key);
  if (!m) continue;
  const channel = Object.keys(index.channels).find((c) => shortName(c) === m[1]);
  if (!channel || !index.channels[channel][m[2]].includes(m[3])) fail(`ws.schema.json: ${key} is not in index.json`);
}

// ------------------------------------------------------------ traffic

const counts = new Map();
const count = (key) => counts.set(key, (counts.get(key) ?? 0) + 1);
let records = 0;
if (trafficFile) {
  const lines = readFileSync(trafficFile, "utf8").split("\n");
  lines.forEach((text, n) => {
    if (!text.trim()) return;
    records++;
    const at = `${trafficFile}:${n + 1}`;
    let rec;
    try {
      rec = JSON.parse(text);
    } catch {
      return fail(`${at}: not JSON`);
    }
    const label = `${rec.src ?? "?"} ${rec.kind}`;
    if (rec.kind === "http") {
      const matches = routesFor(rec.path ?? "").filter((r) => r.method === rec.method);
      if (!matches.length) return fail(`${at}: ${rec.method} ${rec.path} is not a documented route`);
      const r = matches[0];
      const defs = httpSchema.$defs[routeKey(r)]?.$defs ?? {};
      count(`http ${routeKey(r)} ${rec.dir === "out" ? "request" : rec.status}`);
      let schema;
      let what;
      if (rec.dir === "out") {
        [schema, what] = [defs.request, "request"];
        if (!schema) return rec.body === null || rec.body === undefined ? undefined : fail(`${at}: ${routeKey(r)} takes no JSON body`);
      } else if (rec.status >= 200 && rec.status < 300) {
        [schema, what] = [defs.response, `${rec.status} response`];
        if (!schema) return rec.body === null ? undefined : fail(`${at}: ${routeKey(r)} answers ${rec.status} with a JSON body it does not document`);
      } else {
        [schema, what] = [defs.error ?? { $ref: "#/$defs/error" }, `${rec.status} error`];
      }
      const errors = withRoot(httpSchema, () => validate(schema, rec.body));
      if (errors.length) fail(`${at}: ${label} ${routeKey(r)} ${what}:\n      ${errors.slice(0, 8).join("\n      ")}`);
    } else if (rec.kind === "ws") {
      const c = index.channels[rec.socket];
      if (!c) return fail(`${at}: ${rec.socket} is not a documented channel`);
      const dir = rec.dir === "out" ? "c2s" : "s2c";
      const t = rec.body?.t;
      count(`ws ${rec.socket} ${dir} ${t}`);
      if (!c[dir].includes(t)) return fail(`${at}: t=${JSON.stringify(t)} is not documented on ${rec.socket} ${dir}`);
      const errors = withRoot(wsSchema, () => validate(wsSchema.$defs[`${shortName(rec.socket)}.${dir}.${t}`], rec.body));
      if (errors.length) fail(`${at}: ${label} ${rec.socket} ${dir} ${t}:\n      ${errors.slice(0, 8).join("\n      ")}`);
    } else {
      fail(`${at}: kind ${JSON.stringify(rec.kind)}`);
    }
  });
}

// ------------------------------------------------------------ report

if (listAll) {
  console.log("routes found (method or * when the code does not say):");
  for (const [key, where] of [...found.routes].sort()) console.log(`  ${key.padEnd(52)} ${where.length}x  e.g. ${where[0]}`);
  console.log("control messages found:");
  for (const [key, where] of [...found.ts].sort()) {
    console.log(`  ${key}`);
    for (const w of where) console.log(`      ${w}`);
  }
  console.log(`messages of this computer's own channels (${[...LOCAL].join(", ")}; not hub API, not checked):`);
  for (const [key, where] of [...found.local].sort()) {
    console.log(`  ${key}`);
    for (const w of where) console.log(`      ${w}`);
  }
}
if (trafficFile) {
  console.log(`traffic: ${records} records`);
  for (const [key, n] of [...counts].sort()) console.log(`  ${String(n).padStart(5)}  ${key}`);
}
const tCount = [...found.ts.keys()].length;
const channelTs = Object.values(index.channels).reduce((n, c) => n + c.c2s.length + c.s2c.length, 0);
if (failures.length) {
  console.error(`\nhub-api check: ${failures.length} problem(s)`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `hub-api check: ok — ${filesScanned} files scanned; ${found.routes.size} distinct route uses and ${tCount} control message types found, all documented ` +
    `(index: ${routes.length} routes, ${Object.keys(index.channels).length} channels, ${channelTs} message types)` +
    (found.local.size ? `; ${found.local.size} message types of this computer's own ${[...LOCAL].join(", ")} left out` : "") +
    (trafficFile ? `; ${records} traffic records valid` : ""),
);
