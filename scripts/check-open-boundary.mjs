#!/usr/bin/env node
/**
 * The open-source boundary (docs/local-aggregator.md §2.1): the open crates
 * and `web/` never reach into the hub — not its code, not its schema, not its
 * environment — and the hub depends on them, never the other way round. The
 * hub (`server-rs/crates/rh-hub`, `droppen-hub` once renamed) is closed and
 * moves to a repository of its own; this check keeps the open tree buildable
 * without it, here (hub present) and in the public repository (hub absent).
 *
 * The hub's side is the hub and the packages that go with it, named after it
 * (`rh-hub-*`, `droppen-hub-*`: `rh-hub-tools`, its white-box e2e and stress
 * runs). They may depend on one another and on the open crates.
 *
 * From `cargo metadata --no-deps` over `server-rs/Cargo.toml` (the declared
 * dependencies are all it needs: no resolution, no network, and Cargo.lock is
 * never rewritten):
 * - no open package depends on the hub's side, as a normal, dev or build
 *   dependency, from any source (path, git or registry);
 * - no open package has a path dependency, or is itself, outside this
 *   repository;
 * - with the hub absent, its side is absent too and nothing names it: cargo
 *   cannot load the workspace when a member or a path dependency points at
 *   it, and the manifests that still name it are listed either way.
 *
 * From the open tree's sources ({@link SCANNED}), comments left out:
 * - no `rh_hub` / `droppen_hub` (the hub's crates, `rh_hub_tools` too) and no
 *   path into them (`../rh-hub/…`, `crates/rh-hub-tools/…`);
 * - no hub environment variable (`RH_HUB_*`, `DROPPEN_HUB_*`) but the open
 *   side's own ({@link OPEN_ENV}),
 * except what {@link ALLOWED} lets a file name, and says why.
 *
 * `desktop/` is not looked at: it is a cargo workspace of its own (the Tauri
 * app), which builds the server from `server-rs/` and, as of this writing,
 * names nothing of the hub.
 *
 *   node scripts/check-open-boundary.mjs        (pnpm check:boundary)
 */
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, realpathSync, statSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** The hub's package names: today's, and the one the rename gives it. */
export const HUB_NAMES = ["rh-hub", "droppen-hub"];
const dashed = (name) => String(name).replace(/_/g, "-");
/** Whether a package is the hub itself. */
export const isTheHub = (name) => HUB_NAMES.includes(dashed(name));
/** Whether a package is on the hub's side: the hub, or one named after it (`rh-hub-tools`). */
export const isHub = (name) => HUB_NAMES.some((hub) => dashed(name) === hub || dashed(name).startsWith(`${hub}-`));

/**
 * Environment variables named like the hub's that are the open side's own:
 * the hub a new sign-in uses (`server-rs/src/api/cloud.rs`) and the hub-API
 * traffic capture (`docs/hub-api/check.mjs --traffic`), each read under its
 * Droppen name first and its name from before the rename after it
 * (`server-rs/src/env.rs`). Exactly the names main reads; a new one is
 * added here with its reason, never by widening the pattern.
 */
export const OPEN_ENV = ["RH_HUB_DEFAULT", "DROPPEN_HUB_DEFAULT", "RH_HUB_API_TRAFFIC", "DROPPEN_HUB_API_TRAFFIC"];

/**
 * Where the open tree's sources are, besides each open package's `src`,
 * `tests`, `benches`, `examples`, `build.rs` and `scripts`.
 */
export const SCANNED = ["web/src", "web/test", "packages/plugin-sdk/src", ".github/workflows", ".github/scripts", "scripts"];

/**
 * Files the scan passes over: this check and its tests, which spell out the
 * very names they look for.
 */
export const NOT_SCANNED = ["scripts/check-open-boundary.mjs", "scripts/check-open-boundary.test.mjs"];

/**
 * What an open file may name of the hub's side anyway: exactly these
 * environment variables, and these matches of the crate and path rules
 * (`rh-hub/` for a path into the hub's crate). Keep it short and say why for
 * each. An entry whose file is gone is reported (drop it then).
 */
export const ALLOWED = [
  // The local stack for the web app's cloud mode (`pnpm dev` against a hub
  // on this computer) starts a hub and configures it: built from
  // `crates/rh-hub` when the tree has it, else a pinned image
  // (`DROPPEN_HUB_IMAGE`). It configures the hub only through the image's
  // documented run interface (docs/hub-api/running-the-hub-image.md),
  // under the names before the rename too.
  {
    file: "server-rs/crates/rh-devkit/scripts/cloud-dev.sh",
    allow: [
      "DROPPEN_HUB_REDIS_URL", "DROPPEN_HUB_BUCKET_DIR", "DROPPEN_HUB_BIND", "DROPPEN_HUB_ORIGIN", "DROPPEN_HUB_SESSION_KEY",
      "DROPPEN_HUB_WEB_DIR", "DROPPEN_HUB_CHAIN_V3", "RH_HUB_CHAIN_V3", "DROPPEN_HUB_SIGNUP", "DROPPEN_HUB_DEV_OPEN_SIGNUP",
      "DROPPEN_HUB_EMAIL_LOG", "DROPPEN_HUB_PUBLIC_URL", "DROPPEN_HUB_TURNSTILE", "DROPPEN_HUB_TURNSTILE_SITE_KEY",
      "DROPPEN_HUB_TURNSTILE_SECRET", "DROPPEN_HUB_TURNSTILE_ORIGIN", "DROPPEN_HUB_RELAY_INSECURE",
      "DROPPEN_HUB_CHANNEL_RELAY", "RH_HUB_CHANNEL_RELAY", "DROPPEN_HUB_IMAGE", "crates/rh-hub",
    ],
  },
  // A test that the relay's names from before the rename do not take the
  // hub's variable: it names it to say it is not the relay's.
  { file: "server-rs/crates/rh-relay/src/config.rs", allow: ["DROPPEN_HUB_ORIGIN"] },
  // CI builds and tests the hub while it lives here: its Docker stage
  // (`crates/rh-hub/Dockerfile`) and its database tests (`RH_HUB_TEST_*`).
  // These jobs go to the hub repository with it.
  { file: ".github/workflows/ci.yml", allow: ["rh-hub/", "RH_HUB_TEST_DATABASE_URL", "RH_HUB_TEST_REDIS_URL", "RH_HUB_TEST_REQUIRE_DB"] },
];

const allowed = (file, text) => ALLOWED.some((a) => a.file === file && a.allow.includes(text));

// ------------------------------------------------------------ comments out

const IDENT_REST = /[A-Za-z0-9_$]/;
const blank = (text) => text.replace(/[^\n]/g, " ");

/**
 * `src` with its comments blanked (newlines kept, so lines still count) and
 * everything else, strings included, as it was. `lang` is "rs", "js" (also
 * TypeScript and JSX) or "sh" (`#` comments: shell, YAML). A lexer good
 * enough to tell comments from strings, not a parser.
 */
export function codeOnly(src, lang) {
  return lang === "rs" ? rustCode(src) : lang === "sh" ? hashCode(src) : jsCode(src);
}

/**
 * Shell and YAML: a `#` that starts a word, outside quotes, comments out the
 * rest of its line (`$#`, `${#a[@]}` and `a#b` are not comments). Quotes end
 * at the line's end at the latest, and an apostrophe right after a word is
 * prose (a YAML plain scalar), so one odd quote costs one line at most. A
 * here-document's lines (`<<EOF` … `EOF`, `<<-'EOF'` …) are what a command
 * reads, not comments: a `#` in them hides nothing.
 */
function hashCode(src) {
  let out = "";
  let heredoc = null;
  for (const line of src.split("\n")) {
    if (heredoc) {
      out += line + "\n";
      if (line.replace(/^\t+/, "").trimEnd() === heredoc) heredoc = null;
      continue;
    }
    let quote = null;
    let i = 0;
    for (; i < line.length; i++) {
      const c = line[i];
      if (quote) {
        if (c === "\\" && quote === '"') i++;
        else if (c === quote) quote = null;
      } else if (c === '"' || (c === "'" && !(i > 0 && /[A-Za-z0-9]/.test(line[i - 1])))) quote = c;
      else if (c === "#" && (i === 0 || /\s/.test(line[i - 1]))) break;
    }
    out += line.slice(0, i) + blank(line.slice(i)) + "\n";
    const opens = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/.exec(line.slice(0, i));
    if (opens) heredoc = opens[2];
  }
  return out.slice(0, -1);
}

function rustCode(src) {
  const n = src.length;
  let out = "";
  let i = 0;
  while (i < n) {
    const c = src[i];
    const next = src[i + 1];
    if (c === "/" && next === "/") {
      const end = src.indexOf("\n", i);
      const stop = end < 0 ? n : end;
      out += blank(src.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === "/" && next === "*") {
      // block comments nest in Rust
      let depth = 0;
      let j = i;
      while (j < n) {
        if (src[j] === "/" && src[j + 1] === "*") {
          depth++;
          j += 2;
        } else if (src[j] === "*" && src[j + 1] === "/") {
          depth--;
          j += 2;
          if (depth === 0) break;
        } else j++;
      }
      out += blank(src.slice(i, j));
      i = j;
      continue;
    }
    // raw strings: r"…", r#"…"#, br"…", cr#"…"#
    const raw = (i === 0 || !IDENT_REST.test(src[i - 1])) && /^(?:b|c)?r(#*)"/.exec(src.slice(i, i + 40));
    if (raw) {
      const close = '"' + raw[1];
      const end = src.indexOf(close, i + raw[0].length);
      const stop = end < 0 ? n : end + close.length;
      out += src.slice(i, stop);
      i = stop;
      continue;
    }
    if (c === '"') {
      let j = i + 1;
      while (j < n && src[j] !== '"') j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, j + 1);
      i = j + 1;
      continue;
    }
    if (c === "'") {
      // a char literal ('x', '\n', '\''), or a lifetime ('a)
      if (next === "\\") {
        const end = src.indexOf("'", i + 3);
        const stop = end < 0 ? n : end + 1;
        out += src.slice(i, stop);
        i = stop;
        continue;
      }
      const width = (src.codePointAt(i + 1) ?? 0) > 0xffff ? 2 : 1;
      if (src[i + 1 + width] === "'") {
        out += src.slice(i, i + 2 + width);
        i += 2 + width;
        continue;
      }
    }
    out += c;
    i++;
  }
  return out;
}

const REGEX_AFTER = new Set(["(", ",", "=", ":", "[", "!", "&", "|", "?", "{", "}", ";", "+", "-", "*", "%", "<", ">", "~", "^"]);
const REGEX_AFTER_WORDS = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "yield", "await", "instanceof"]);

function jsCode(src) {
  const n = src.length;
  let out = "";
  let i = 0;
  // the last token that decides whether a "/" starts a regex: a word, or a punctuator
  let last = null;
  // one entry per template literal whose `${…}` is open: the "{" depth inside it
  const templates = [];

  /** From just after a "`" or a template's closing "}", up to and including the end or "${". */
  function templateText() {
    while (i < n) {
      if (src[i] === "\\") {
        out += src.slice(i, i + 2);
        i += 2;
      } else if (src[i] === "`") {
        out += "`";
        i++;
        last = { word: false, v: "`" };
        return;
      } else if (src[i] === "$" && src[i + 1] === "{") {
        out += "${";
        i += 2;
        templates.push(0);
        last = { word: false, v: "{" };
        return;
      } else out += src[i++];
    }
  }

  while (i < n) {
    const c = src[i];
    const next = src[i + 1];
    if (c === " " || c === "\t" || c === "\r" || c === "\n") {
      out += c;
      i++;
      continue;
    }
    if (c === "/" && next === "/") {
      const end = src.indexOf("\n", i);
      const stop = end < 0 ? n : end;
      out += blank(src.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === "/" && next === "*") {
      const end = src.indexOf("*/", i + 2);
      const stop = end < 0 ? n : end + 2;
      out += blank(src.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === '"' || c === "'") {
      // An apostrophe right after a word is prose in JSX text, not a string.
      if (c === "'" && i > 0 && IDENT_REST.test(src[i - 1])) {
        out += c;
        i++;
        continue;
      }
      let j = i + 1;
      while (j < n && src[j] !== c && src[j] !== "\n") j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, j + 1);
      i = j + 1;
      last = { word: false, v: "str" };
      continue;
    }
    if (c === "`") {
      out += c;
      i++;
      templateText();
      continue;
    }
    if (templates.length && (c === "{" || c === "}")) {
      if (c === "{") templates[templates.length - 1]++;
      else if (templates[templates.length - 1] === 0) {
        // the end of a `${…}`: back into the template's text
        templates.pop();
        out += c;
        i++;
        templateText();
        continue;
      } else templates[templates.length - 1]--;
    }
    if (c === "/") {
      // `</p>` closes a JSX element; `a < /x/` is not worth telling apart from it
      const regex = !last || (last.word ? REGEX_AFTER_WORDS.has(last.v) : REGEX_AFTER.has(last.v) && last.v !== "<");
      if (regex) {
        let j = i + 1;
        let inClass = false;
        while (j < n && src[j] !== "\n") {
          if (src[j] === "\\") {
            j += 2;
            continue;
          }
          if (src[j] === "[") inClass = true;
          else if (src[j] === "]") inClass = false;
          else if (src[j] === "/" && !inClass) break;
          j++;
        }
        j++;
        while (j < n && /[a-z]/.test(src[j])) j++;
        out += src.slice(i, j);
        i = j;
        last = { word: false, v: "regex" };
        continue;
      }
    }
    if (/[A-Za-z_$0-9]/.test(c)) {
      let j = i + 1;
      while (j < n && IDENT_REST.test(src[j])) j++;
      last = { word: /[A-Za-z_$]/.test(c), v: src.slice(i, j) };
      out += src.slice(i, j);
      i = j;
      continue;
    }
    last = { word: false, v: c };
    out += c;
    i++;
  }
  return out;
}

// ------------------------------------------------------------ sources

// The hub's crates: `rh_hub`, `rh_hub_tools` and the `droppen_` ones.
const HUB_CRATE = /(?<![A-Za-z0-9_])(?:rh|droppen)_hub(?:_[A-Za-z0-9_]+)?(?![A-Za-z0-9_])/g;
// A path into the hub's side: a directory named after it, however it is
// reached (`../../rh-hub/src/plans.rs`, `{}/../rh-hub/data`,
// `crates/rh-hub-tools/src`); the last segment of a path
// (`crates/rh-hub`, `concat!(…, "/../rh-hub")`); or the segment a path is
// built with (`Path::new(root).join("rh-hub")`, `join(root, "rh-hub")`). A
// file or a binary named after it (`rh-hub-stress.rs`,
// `target/debug/rh-hub-stress`) is not a path into it, and neither is the
// page's documented `<meta name="rh-hub">` marker.
const HUB_PATH = new RegExp(
  [
    String.raw`(?<![A-Za-z0-9_-])(?:rh|droppen)-hub(?:-[a-z]+)?\/`,
    String.raw`crates\/(?:rh|droppen)-hub(?:-[a-z]+)?(?![A-Za-z0-9_.\/-])`,
    String.raw`(?<!target\/(?:debug|release)\/)(?<=\/)(?:rh|droppen)-hub(?:-[a-z]+)?(?![A-Za-z0-9_.\/-])`,
    String.raw`(?<=(?:join|push|resolve|new|from)\((?:[^()]*,\s*)?["'\x60])(?:rh|droppen)-hub(?:-[a-z]+)?(?=["'\x60])`,
  ].join("|"),
  "g",
);
const HUB_ENV = /(?<![A-Za-z0-9_])(?:RH|DROPPEN)_HUB_[A-Za-z0-9_]*/g;

const lineAt = (text, at) => text.slice(0, at).split("\n").length;

/**
 * What `src` (the file `file`, relative to the repository) has of the hub, as
 * readable lines. The {@link ALLOWED} matches it passes over go into `used`
 * (`file` and the text, a newline between).
 */
export function sourceProblems(src, lang, file, used = new Set()) {
  const code = codeOnly(src, lang);
  const problems = [];
  const rule = (re, why, open = () => false) => {
    for (const m of code.matchAll(re)) {
      if (open(m[0])) continue;
      if (allowed(file, m[0])) {
        used.add(`${file}\n${m[0]}`);
        continue;
      }
      problems.push(`${file}:${lineAt(code, m.index)}: \`${m[0]}\` — ${why}, in code`);
    }
  };
  rule(HUB_CRATE, "the hub's crate");
  rule(HUB_PATH, "a path into the hub's source");
  rule(HUB_ENV, "the hub's environment", (name) => OPEN_ENV.includes(name));
  return problems;
}

/** The lexer a file is read with, by its name; null for a file not scanned. */
export function langOf(path) {
  if (path.endsWith(".rs")) return "rs";
  if (/\.(?:[cm]?[jt]sx?)$/.test(path)) return "js";
  if (/\.(?:sh|bash|ya?ml|py)$/.test(path)) return "sh";
  return null;
}

function sourceFiles(dir, test, out = []) {
  if (!existsSync(dir)) return out;
  if (!statSync(dir).isDirectory()) {
    if (test(dir)) out.push(dir);
    return out;
  }
  for (const name of readdirSync(dir).sort()) {
    if (name === "node_modules" || name === "target") continue;
    sourceFiles(join(dir, name), test, out);
  }
  return out;
}


// ------------------------------------------------------------ manifests

/** A name on the hub's side (`rh-hub`, `crates/rh-hub-tools`, `rh_hub`), but not a file name (`rh-hub-stress.rs`). */
const HUB_SIDE_NAME = /(?<![A-Za-z0-9_-])(?:rh|droppen)[-_]hub(?:[-_][A-Za-z0-9_-]*[A-Za-z0-9_])?(?![A-Za-z0-9_.-])/;

/**
 * Lines of a Cargo.toml (`text`) that name the hub's side outside comments,
 * as `{line, text}`. Target tables (`[[bin]]`, `[lib]`, …) name binaries and
 * files, not packages, and are passed over: `rh-hub-stress` was the open
 * devkit's binary before it went to `rh-hub-tools`.
 */
export function manifestMentions(text) {
  // TOML comments out, strings kept: "…", '…', """…""", '''…'''
  let code = "";
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    if (c === "#") {
      const end = text.indexOf("\n", i);
      const stop = end < 0 ? n : end;
      code += blank(text.slice(i, stop));
      i = stop;
      continue;
    }
    if (c === '"' || c === "'") {
      const triple = text.startsWith(c.repeat(3), i);
      const close = triple ? c.repeat(3) : c;
      let j = i + close.length;
      while (j < n && !text.startsWith(close, j) && (triple || text[j] !== "\n")) j += c === '"' && text[j] === "\\" ? 2 : 1;
      const stop = Math.min(n, j + close.length);
      code += text.slice(i, stop);
      i = stop;
      continue;
    }
    code += c;
    i++;
  }
  const raw = text.split("\n");
  const found = [];
  let target = false;
  code.split("\n").forEach((line, at) => {
    const header = /^\s*\[(\[?)\s*([^\]]*?)\s*\]\]?\s*$/.exec(line);
    if (header) target = header[1] === "[" ? /^(?:bin|test|bench|example)$/.test(header[2]) : header[2] === "lib";
    else if (target) return;
    if (HUB_SIDE_NAME.test(line)) found.push({ line: at + 1, text: raw[at].trim() });
  });
  return found;
}

// ------------------------------------------------------------ cargo metadata

/**
 * `p` with its symbolic links resolved, as far as it exists (cargo and the
 * repository may name one directory two ways, `/tmp` and `/private/tmp`).
 */
const real = (p) => {
  let head = resolve(p);
  const tail = [];
  while (!existsSync(head) && dirname(head) !== head) {
    tail.unshift(basename(head));
    head = dirname(head);
  }
  try {
    head = realpathSync(head);
  } catch {
    // left as it is
  }
  return join(head, ...tail);
};
const inside = (p, root) => {
  const rel = relative(real(root), real(p));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
};

/**
 * What `cargo metadata` (`meta`, format 1) says against the boundary, for the
 * repository at `root`, as readable lines. Every local package (a workspace
 * member, or another path package) but the hub's side is checked.
 */
export function metadataProblems(meta, root) {
  const problems = [];
  const show = (p) => (inside(p, root) ? relative(real(root), real(p)) : p);
  for (const pkg of meta.packages) {
    if (pkg.source != null || isHub(pkg.name)) continue;
    const where = show(pkg.manifest_path);
    if (!inside(pkg.manifest_path, root)) problems.push(`${where}: ${pkg.name} is outside this repository`);
    for (const dep of pkg.dependencies ?? []) {
      const kind = dep.kind ?? "normal";
      const as = `${kind} dependency ${dep.rename ? `${dep.rename} (${dep.name})` : dep.name}${dep.target ? ` for ${dep.target}` : ""}`;
      if (isHub(dep.name)) problems.push(`${where}: ${pkg.name} has a ${as} — the hub's side depends on the open crates, never the other way round`);
      else if (dep.path != null && !inside(dep.path, root)) problems.push(`${where}: ${pkg.name}'s ${as} is a path outside this repository (${dep.path})`);
    }
  }
  return problems;
}

function cargoMetadata(root) {
  const cargo = process.env.CARGO || "cargo";
  const run = spawnSync(cargo, ["metadata", "--format-version", "1", "--no-deps", "--manifest-path", join(root, "server-rs", "Cargo.toml")], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  if (run.error) return { error: `cannot run ${cargo}: ${run.error.message}` };
  if (run.status !== 0) return { error: run.stderr.trim() || `${cargo} metadata exited ${run.status}` };
  return { meta: JSON.parse(run.stdout) };
}

// ------------------------------------------------------------ the check

/** The whole check over the repository at `root`: `{problems, notes}`, as readable lines. */
export function checkOpenBoundary(root, { metadata = cargoMetadata } = {}) {
  const problems = [];
  const notes = [];
  const crates = join(root, "server-rs", "crates");
  // the hub's side on disk (`crates/rh-hub`, `crates/rh-hub-tools`), in the workspace or not
  const hubDirs = (existsSync(crates) ? readdirSync(crates).sort() : [])
    .filter((name) => isHub(name) && existsSync(join(crates, name, "Cargo.toml")))
    .map((name) => join(crates, name));
  const { meta, error } = metadata(root);
  const side = meta ? meta.packages.filter((p) => p.source == null && isHub(p.name)).map((p) => p.name) : [];
  const hubHere = side.some(isTheHub) || hubDirs.some((dir) => isTheHub(basename(dir)));

  // the open packages' directories: from cargo, or from the tree when cargo cannot load it
  let open;
  if (meta) {
    problems.push(...metadataProblems(meta, root));
    const local = meta.packages.filter((p) => p.source == null);
    open = local.filter((p) => !isHub(p.name) && inside(p.manifest_path, root)).map((p) => ({ name: p.name, dir: dirname(p.manifest_path) }));
    notes.push(`${open.length} open packages (${open.map((p) => p.name).join(", ")}): no dependency of any kind on the hub's side, no path outside the repository`);
    const unlisted = hubDirs.filter((dir) => !side.some((name) => dashed(name) === dashed(basename(dir))));
    if (side.length) notes.push(`the hub's side is in this tree (${side.join(", ")}) and nothing open depends on it`);
    if (unlisted.length) notes.push(`${unlisted.map((dir) => relative(root, dir)).join(", ")} on disk but not in the workspace`);
    if (!side.length && !unlisted.length) notes.push("the hub is not in this tree (the public repository's shape)");
  } else {
    problems.push(`cargo metadata cannot load server-rs/Cargo.toml:\n      ${error.split("\n").join("\n      ")}`);
    open = [{ name: "remote-hand-server", dir: join(root, "server-rs") }];
    for (const name of existsSync(crates) ? readdirSync(crates).sort() : []) {
      if (!isHub(name) && existsSync(join(crates, name, "Cargo.toml"))) open.push({ name, dir: join(crates, name) });
    }
  }

  // with the hub absent, its side goes too, and every manifest that still names it (cargo stops at the first)
  if (!hubHere) {
    const left = [...new Set([...side, ...hubDirs.map((dir) => basename(dir))].map(dashed))];
    if (left.length) problems.push(`${left.join(", ")}: on the hub's side, but the hub is not in this tree; it goes with the hub`);
    const manifests = [join(root, "server-rs", "Cargo.toml"), ...open.filter((p) => p.dir !== join(root, "server-rs")).map((p) => join(p.dir, "Cargo.toml"))];
    for (const file of manifests) {
      if (!existsSync(file)) continue;
      for (const m of manifestMentions(readFileSync(file, "utf8"))) {
        problems.push(`${relative(root, file)}:${m.line}: names the hub's side, and the hub is not in this tree: ${m.text}`);
      }
    }
  }

  const dirs = [
    ...open.flatMap(({ dir }) => ["src", "tests", "benches", "examples", "build.rs", "scripts"].map((sub) => join(dir, sub))),
    ...SCANNED.map((dir) => join(root, dir)),
  ];
  let scanned = 0;
  const used = new Set();
  for (const file of new Set(dirs.flatMap((dir) => sourceFiles(dir, (p) => langOf(p) !== null)))) {
    const rel = relative(root, file).split("\\").join("/");
    if (NOT_SCANNED.includes(rel)) continue;
    scanned++;
    problems.push(...sourceProblems(readFileSync(file, "utf8"), langOf(file), rel, used));
  }
  notes.push(
    `${scanned} source files (the open packages', ${SCANNED.join(", ")}): no hub crate, path or environment in code ` +
      `(open variables: ${OPEN_ENV.join(", ")}; allowed in ${ALLOWED.length} files, each with its reason)`,
  );
  for (const a of ALLOWED) {
    if (!existsSync(join(root, a.file))) notes.push(`ALLOWED lists ${a.file}, which is no longer here: drop the entry`);
    else {
      const unused = a.allow.filter((text) => !used.has(`${a.file}\n${text}`));
      if (unused.length) notes.push(`ALLOWED lets ${a.file} name ${unused.join(", ")}, which it does not here`);
    }
  }
  return { problems, notes };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const { problems, notes } = checkOpenBoundary(root);
  if (problems.length) {
    console.error(`open-source boundary: ${problems.length} problem(s)`);
    for (const p of problems) console.error(`  - ${p}`);
    console.error(
      "\nThe open crates and web/ may not reach into the hub: no dependency on it, no path outside this repository, " +
        "none of its code, schema or environment (docs/local-aggregator.md §2.1). Talk to it through rh-proto and the " +
        "documented hub API (docs/hub-api/); a hub variable the open side really reads goes in OPEN_ENV, with a reason.",
    );
    process.exit(1);
  }
  console.log("open-source boundary: ok");
  for (const note of notes) console.log(`  ${note}`);
}
