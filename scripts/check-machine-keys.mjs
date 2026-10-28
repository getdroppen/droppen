#!/usr/bin/env node
/**
 * Everything the web app asks a machine for is addressed by the machine
 * (`docs/local-aggregator.md` §6.1, §6.7 M-0/M-1): a transport, a typed
 * client, a query cache entry, a feed, a stored draft. The page-wide versions
 * from before — keys that name no machine — are gone, and this check keeps
 * new code in `web/src` from reaching for what replaced them wrongly:
 *
 * - **home**: `homeMachine()` only where the person's own things are decided
 *   (the shells, the root provider, the page login, plugins' feed) — a view
 *   takes its machine from `useMachine()` or a `SessionRef`.
 * - **singletons**: a `FeedHub` or a session feed pool is made per machine,
 *   with `perMachine(…)`, never once for the page.
 * - **query keys**: every `queryKey` (and every key handed to
 *   `setQueryData`/`getQueryData`/…) is a call — `mq(machine, …)` or a
 *   builder that returns one; nothing invalidates, resets or clears every
 *   machine's queries at once (no filter, a filter with no `queryKey`,
 *   `queryClient.clear()`).
 * - **storage**: a `localStorage`/`sessionStorage` key is either one of this
 *   browser's own (`DEVICE_KEYS`) or built per machine with
 *   `machineStorageKey(machine, …)` (`web/src/lib/machines/storage.ts`); the
 *   device copy's IndexedDB keys likewise go through `machineCacheKey`.
 *
 * A regular-expression check rather than a lint plugin: the repository has no
 * linter, and these rules are about names, not types. It is a lint, not a
 * proof: it catches the common un-keyed forms written out where they are
 * used, and not a key or filter held in a variable, a name reached through an
 * alias, a namespace or a dynamic import, or a query key builder its
 * first-definition heuristic misjudges (`buildsMq`).
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

function files(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...files(path));
    else if (/\.(ts|tsx)$/.test(name) && !/\.d\.ts$/.test(name)) out.push(path);
  }
  return out;
}

/**
 * The text with comments blanked out, every offset and line kept. String and
 * template literals are matched first and kept, so a `//` or `/*` inside one
 * ("a // b", "src/*.ts") is not taken for a comment. A regular expression
 * literal is not recognised, so a quote or backtick inside one can throw the
 * strings after it off; a `//` after `\` or `:` (`/https?:\/\//`) is never
 * taken for a comment.
 */
function code(text) {
  const blank = (m) => m.replace(/[^\n]/g, " ");
  return text.replace(
    /("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\[\s\S])*`)|\/\*[\s\S]*?\*\/|(?<![:\\])\/\/[^\n]*/g,
    (m, literal) => (literal ? m : blank(m)),
  );
}

/** The text of the object literal opening at `from` (a `{`), braces and all, or `null`. */
function objectAt(text, from) {
  let depth = 0;
  for (let i = from; i < text.length; i++) {
    if (text[i] === "{") depth++;
    else if (text[i] === "}" && --depth === 0) return text.slice(from, i + 1);
  }
  return null;
}

const lineOf = (text, index) => text.slice(0, index).split("\n").length;

/**
 * Keys of this browser rather than of a machine: the login, the device's
 * own settings and layout, the hub account. Exact keys, or prefixes ending
 * in `.`. Anything else under `rh.` is a machine's, and goes through
 * `machineStorageKey`.
 */
export const DEVICE_KEYS = [
  "rh.auth.token",
  "rh.theme",
  "rh.fontSize",
  "rh.lang",
  "rh.hub.",
  "rh.relays.manual",
  "rh.cloud.signout-failures",
  // A browser just signed out and removed that the account still lists, for the sign-in screen (sessionStorage).
  "rh.cloud.signout-still-listed",
  // The pairing screen's tab to come back to after its "sign in again" (sessionStorage).
  "rh.cloud.pairing-resume",
  // This tab's account was just deleted from its settings, for the sign-in screen to say once (sessionStorage).
  "rh.cloud.account-deleted",
  "rh.panel.",
  "rh.sessions.backgroundPanelOpen",
  "rh.files.showHidden",
  // The download page's tab chosen in this browser tab (sessionStorage).
  "rh.download.os",
  // The Agents & accounts tab this browser used last.
  "rh.settings.agents-tab",
  // Not storage: the names of a machine's index and status streams.
  "rh.index",
  "rh.status",
];

const deviceKey = (key) => DEVICE_KEYS.some((allowed) => (allowed.endsWith(".") ? key.startsWith(allowed) : key === allowed));

/** Where `homeMachine()` may be called (paths under `web/src`; a trailing `/` is a directory). */
const HOME_ALLOWED = [
  "lib/machines/",
  "lib/auth.ts",
  "app/root-layout.tsx",
  "app/global-feed.tsx",
  "features/cloud/",
];

/** The files that define the storage keys of machines. */
const STORAGE_HELPERS = ["lib/machines/storage.ts"];

/**
 * Users of the device copy that key their records by machine themselves: the
 * cloud mirror files every record under `stream/<account>/<machine>/…`
 * (`streamPrefix`, `metaKey`).
 */
const CACHE_KEYED_ELSEWHERE = ["lib/local-cache.ts", "lib/hub/mirror.ts"];

/** The text of the second argument of a call whose arguments start at `from`, or `null`. */
function secondArgument(text, from) {
  let depth = 0;
  for (let i = from; i < text.length; i++) {
    const c = text[i];
    if (c === "(" || c === "[" || c === "{") depth++;
    else if (c === ")" || c === "]" || c === "}") {
      if (depth === 0) return null;
      depth--;
    } else if (c === "," && depth === 0) return text.slice(i + 1).trimStart();
  }
  return null;
}

const allowedAt = (list, rel) => list.some((entry) => (entry.endsWith("/") ? rel.startsWith(entry) : rel === entry));

/** Generic arguments, one level of nesting deep: `<A>`, `<Record<K, V>>`. */
const GENERIC = String.raw`(?:<(?:[^<>]|<[^<>]*>)*>)?`;

/** Every problem under `root` (a checkout), as readable lines. */
export function checkMachineKeys(root) {
  const src = join(root, "web/src");
  const problems = [];
  const sources = files(src).map((path) => {
    const text = readFileSync(path, "utf8");
    return { path, rel: relative(src, path).split(sep).join("/"), text, code: code(text) };
  });
  const say = (file, index, message) => problems.push(`web/src/${file.rel}:${lineOf(file.code, index)}: ${message}`);

  // Where each key builder is defined, for the query-key rule.
  const definitions = new Map();
  for (const file of sources) {
    for (const m of file.code.matchAll(/(?:function\s+([A-Za-z_$][\w$]*)\s*\(|(?:const|let)\s+([A-Za-z_$][\w$]*)\s*=)/g)) {
      const name = m[1] ?? m[2];
      if (!definitions.has(name)) definitions.set(name, file.code.slice(m.index, m.index + 400));
    }
  }
  const buildsMq = (name) => name === "mq" || name === "machineQueries" || /\bmq\s*\(/.test(definitions.get(name) ?? "");

  for (const file of sources) {
    const text = file.code;

    // The home machine, named outside the few places that decide it.
    if (!allowedAt(HOME_ALLOWED, file.rel)) {
      for (const m of text.matchAll(/\bhomeMachine\s*\(/g)) {
        say(file, m.index, "names the home machine — a view's machine is `useMachine()` (or its `SessionRef`'s)");
      }
    }

    // Per-machine singletons.
    if (!/\bperMachine\s*\(/.test(text)) {
      for (const m of text.matchAll(/\bnew\s+FeedHub\s*\(|(?<!function\s)\bcreateSessionFeedPool\s*\(/g)) {
        say(file, m.index, "a feed hub or session feed pool for the whole page — make one per machine with `perMachine(…)`");
      }
    }

    // Query keys.
    for (const m of text.matchAll(/\bqueryKey\s*:\s*/g)) {
      const after = text.slice(m.index + m[0].length);
      const call = /^([A-Za-z_$][\w$]*)\s*\(/.exec(after);
      if (!call) say(file, m.index, "a query key written out by hand — build it with `mq(machine, …)`");
      else if (!buildsMq(call[1])) say(file, m.index, `the query key builder \`${call[1]}\` does not build its keys with \`mq(machine, …)\``);
    }
    const keyed = new RegExp(String.raw`\b(?:setQueryData|getQueryData|ensureQueryData|setQueriesData|getQueriesData)\s*${GENERIC}\(\s*`, "g");
    for (const m of text.matchAll(keyed)) {
      const after = text.slice(m.index + m[0].length);
      const call = /^([A-Za-z_$][\w$]*)\s*\(/.exec(after);
      if (/^\{/.test(after)) continue; // a filter object: its `queryKey:` is checked above
      if (!call) say(file, m.index, "a query key written out by hand — build it with `mq(machine, …)`");
      else if (!buildsMq(call[1])) say(file, m.index, `the query key builder \`${call[1]}\` does not build its keys with \`mq(machine, …)\``);
    }
    // With no filter, a filter with no `queryKey` (only a `predicate`, a
    // `type`), or the whole cache cleared.
    const everyMachine = "every machine's queries at once — `invalidateMachine(client, machine)`";
    const filtered = new RegExp(String.raw`\b(?:invalidateQueries|removeQueries|cancelQueries|refetchQueries|resetQueries)\s*${GENERIC}\(\s*(\)|\{)`, "g");
    for (const m of text.matchAll(filtered)) {
      if (m[1] === ")") say(file, m.index, everyMachine);
      else if (!/\bqueryKey\b/.test(objectAt(text, m.index + m[0].length - 1) ?? "")) say(file, m.index, everyMachine);
    }
    for (const m of text.matchAll(/\b(?:\w*[qQ]ueryClient|qc|client)\s*\.\s*clear\s*\(\s*\)|\bgetQueryCache\s*\(\s*\)\s*\.\s*clear\s*\(/g)) {
      say(file, m.index, everyMachine);
    }

    // Storage keys.
    if (!allowedAt(STORAGE_HELPERS, file.rel)) {
      for (const m of text.matchAll(/["'`](rh\.[A-Za-z][^"'`$]*)/g)) {
        if (!deviceKey(m[1])) say(file, m.index, `the storage key "${m[1]}…" names no machine — build it with \`machineStorageKey(machine, …)\``);
      }
      // `rh.` and whatever follows it, from a variable: `rh.${name}`, "rh." + name.
      for (const m of text.matchAll(/`rh\.\$\{|(["'`])rh\.\1\s*\+/g)) {
        say(file, m.index, "a storage key built from `rh.` and a name names no machine — build it with `machineStorageKey(machine, …)`");
      }
      for (const m of text.matchAll(/\b(?:localStorage|sessionStorage)\s*\.\s*(?:getItem|setItem|removeItem)\s*\(\s*["'`]([^"'`$]*)/g)) {
        if (!m[1].startsWith("rh.")) say(file, m.index, `the storage key "${m[1]}" is not one of this browser's (\`DEVICE_KEYS\`) — a machine's goes through \`machineStorageKey(machine, …)\``);
      }
    }

    // The device copy (IndexedDB, `lib/local-cache.ts`): a machine's records
    // are under `machineCacheKey(machine, …)`, never a key written out.
    if (!allowedAt(CACHE_KEYED_ELSEWHERE, file.rel) && /\b(?:cacheTransact|cacheGet|cacheSet)\s*[<(]/.test(text)) {
      if (!/\bmachineCacheKey\s*\(/.test(text)) {
        const at = /\b(?:cacheTransact|cacheGet|cacheSet)\s*[<(]/.exec(text);
        say(file, at.index, "uses the device copy without `machineCacheKey(machine, …)` — its records are a machine's");
      }
      const written = "a device copy key written out by hand — build it with `machineCacheKey(machine, …)`";
      for (const m of text.matchAll(/\bstore\s*\.\s*(?:get|delete)\s*\(\s*["'`]|\bcache(?:Get|Set)\s*(?:<[^>]*>)?\(\s*["'`]/g)) {
        say(file, m.index, written);
      }
      // `store.put(value, key)`: the key is the second argument.
      for (const m of text.matchAll(/\bstore\s*\.\s*put\s*\(/g)) {
        const key = secondArgument(text, m.index + m[0].length);
        if (key !== null && /^["'`]/.test(key)) say(file, m.index, written);
      }
    }
  }
  return problems;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const problems = checkMachineKeys(root);
  if (problems.length) {
    console.error(problems.join("\n"));
    console.error(`\n${problems.length} un-keyed access(es); see scripts/check-machine-keys.mjs.`);
    process.exit(1);
  }
  console.log("machine keys: ok");
}
