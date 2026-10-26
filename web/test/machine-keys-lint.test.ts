// scripts/check-machine-keys.mjs keeps un-keyed query and storage keys from
// creeping back into web/src. These cases run it against a scratch tree, and
// against the real one. It is a lint, not a proof: it catches the common
// un-keyed forms, not a key held in a variable, an aliased or dynamic import,
// and the like.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
// @ts-expect-error — a plain .mjs script, without types.
import { checkMachineKeys } from "../../scripts/check-machine-keys.mjs";

function tree(files: Record<string, string>): string {
  const root = mkdtempSync(join(tmpdir(), "droppen-machine-keys-"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(root, "web/src", path)), { recursive: true });
    writeFileSync(join(root, "web/src", path), text);
  }
  return root;
}

/** `file:line` of every problem, sorted. */
function where(root: string): string[] {
  return (checkMachineKeys(root) as string[]).map((p) => p.split(": ")[0]!.replace(/^web\/src\//, "")).sort();
}

test("the real tree is clean", () => {
  assert.deepEqual(checkMachineKeys(resolve(import.meta.dirname, "../..")), []);
});

test("query keys are built with mq(), never written out, and nothing invalidates every machine", () => {
  const root = tree({
    "lib/machines/query.ts": "export function mq(machine, ...parts) { return ['m', machine, ...parts]; }\nexport function machineQueries(machine) { return ['m', machine]; }\n",
    "features/keys.ts": 'import { mq } from "@/lib/machines/query";\nexport const goodKey = (machine) => mq(machine, "good");\nexport const badKey = (machine) => ["bad", machine];\nconst BARE = ["bare"];\n',
    "features/view.tsx": [
      'useQuery({ queryKey: mq(machine, "prefs"), queryFn });',
      "useQuery({ queryKey: goodKey(machine), queryFn });",
      'useQuery({ queryKey: ["prefs"], queryFn });',
      "useQuery({ queryKey: BARE, queryFn });",
      "useQuery({ queryKey: badKey(machine), queryFn });",
      'client.setQueryData<Prefs>(["prefs"], data);',
      "client.getQueryData<Record<string, number>>(BARE);",
      'client.setQueryData(mq(machine, "prefs"), data);',
      "void client.invalidateQueries();",
      "void client.invalidateQueries({ queryKey: machineQueries(machine) });",
      "",
    ].join("\n"),
  });
  assert.deepEqual(where(root), ["features/view.tsx:3", "features/view.tsx:4", "features/view.tsx:5", "features/view.tsx:6", "features/view.tsx:7", "features/view.tsx:9"]);
});

test("a filter with no query key, or clearing the cache, reaches every machine's queries", () => {
  const root = tree({
    "lib/machines/query.ts": "export function machineQueries(machine) { return ['m', machine]; }\n",
    "features/view.tsx": [
      "void queryClient.invalidateQueries({ predicate: (q) => q.state.data === undefined });",
      'void queryClient.resetQueries({ type: "all" });',
      "queryClient.clear();",
      "queryClient.getQueryCache().clear();",
      "void client.refetchQueries<Prefs>({",
      '  type: "active",',
      "});",
      // Allowed: scoped to one machine, and other things' own clear().
      "void queryClient.invalidateQueries({ queryKey: machineQueries(machine), predicate: (q) => !q.state.data });",
      "void queryClient.removeQueries({",
      "  queryKey: machineQueries(machine),",
      "});",
      "this.channels.clear();",
      "",
    ].join("\n"),
  });
  assert.deepEqual(where(root), ["features/view.tsx:1", "features/view.tsx:2", "features/view.tsx:3", "features/view.tsx:4", "features/view.tsx:5"]);
});

test("only the shells, the root and the page's own login name the home machine", () => {
  const root = tree({
    "features/sessions/view.tsx": "const machine = homeMachine();\n",
    "app/root-layout.tsx": "<MachineProvider machine={homeMachine()} />;\n",
    "features/cloud/cloud-app.tsx": "openHubMachine(machine, transport); homeMachine();\n",
    "lib/auth.ts": "const pageApi = () => apiFor(homeMachine());\n",
  });
  assert.deepEqual(where(root), ["features/sessions/view.tsx:1"]);
});

test("a feed hub or session feed pool is made per machine", () => {
  const root = tree({
    "app/page-feed.ts": "export const feedHub = new FeedHub(deps);\n",
    "app/machine-feed.ts": "export const feedHubFor = perMachine((machine) => new FeedHub(depsFor(machine)));\n",
    "features/pool.ts": "const pool = createSessionFeedPool(deps);\n",
    "features/session-feed-pool.ts": "export function createSessionFeedPool(deps) {}\n",
  });
  assert.deepEqual(where(root), ["app/page-feed.ts:1", "features/pool.ts:1"]);
});

test("a storage key is this browser's own, or built per machine", () => {
  const root = tree({
    "lib/machines/storage.ts": "export const machineStorageKey = (machine, name) => machine === 'local' ? `rh.${name}` : `rh.m/${machine}/${name}`;\n",
    "features/theme.ts": 'const KEY = "rh.theme";\nlocalStorage.getItem("rh.panel.width.sessions");\n',
    "features/draft.ts": "export const draftKey = (source, id) => `rh.draft.${source}.${id}`;\n",
    "features/queue.ts": 'localStorage.setItem("rh.queue.claude.a", "[]");\nsessionStorage.getItem("pending");\n',
    "features/ok.ts": 'readMachineItem(localStorage, machine, "files.lastDir");\nconst stream = "rh.index";\n',
  });
  assert.deepEqual(where(root), ["features/draft.ts:1", "features/queue.ts:1", "features/queue.ts:2"]);
});

test("a storage key built from `rh.` and a name is caught too", () => {
  const root = tree({
    "lib/machines/storage.ts": 'const LEGACY = "rh.";\nexport const legacyKey = (name) => `rh.${name}`;\n',
    "features/template.ts": "export const key = (name) => `rh.${name}`;\n",
    "features/plus.ts": 'localStorage.getItem("rh." + name);\nsessionStorage.setItem(\'rh.\' + name, value);\n',
    // Allowed: a device key's prefix, with the rest from a variable.
    "features/panel.ts": "localStorage.setItem(`rh.panel.${collapseKey}`, next);\n",
  });
  assert.deepEqual(where(root), ["features/plus.ts:1", "features/plus.ts:2", "features/template.ts:1"]);
});

test("a `//` or `/*` inside a string is not a comment", () => {
  const root = tree({
    "features/slashes.ts": [
      'const sep = " // "; localStorage.setItem("rh.queue.claude.a", "[]");',
      'const help = `don\'t // stop`; localStorage.getItem("rh.queue.claude.z");',
      "",
    ].join("\n"),
    "features/glob.ts": ['const glob = "src/*.ts";', 'localStorage.getItem("rh.draft.claude.b");', "/* a note */", ""].join("\n"),
    // Comments are still comments, and these are not comments at all.
    "features/ok.ts": [
      '// localStorage.getItem("rh.draft.claude.c");',
      '/* localStorage.getItem("rh.draft.claude.d"); */',
      'const url = "https://example.com/rh.x"; // "rh.draft.claude.e"',
      'const scheme = /^https?:\\/\\//; const theme = localStorage.getItem("rh.theme");',
      "",
    ].join("\n"),
  });
  assert.deepEqual(where(root), ["features/glob.ts:2", "features/slashes.ts:1", "features/slashes.ts:2"]);
});

test("the device copy's records are a machine's", () => {
  const root = tree({
    "lib/local-cache.ts": 'export function cacheGet(key) {}\nstore.put(value, "kv");\n',
    "lib/hub/mirror.ts": "cacheTransact('readwrite', (store) => store.put(record, streamRecordKey(a, m, h)));\n",
    "features/unkeyed.ts": 'void cacheTransact("readwrite", (store) => store.put(rows, key));\n',
    "features/written.ts": 'machineCacheKey(machine, "x");\nvoid cacheTransact("readwrite", (store) => { store.put(rows, "sessions"); store.get("feeds"); });\ncacheGet("x");\n',
    "features/keyed.ts": 'void cacheTransact("readwrite", (store) => store.put(rows, machineCacheKey(machine, "sessions")));\n',
  });
  assert.deepEqual(where(root), ["features/unkeyed.ts:1", "features/written.ts:2", "features/written.ts:2", "features/written.ts:3"]);
});
